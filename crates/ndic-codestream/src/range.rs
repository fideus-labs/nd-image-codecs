//! Byte-range plans: thumbnails, planes, and regions from plain HTTP
//! `Range:` requests (`docs/architecture/range-access.md`).
//!
//! A [`RangeIndex`] answers "which bytes do I fetch?" without decoding
//! anything. It is built either from an `htj2k` chunk's coefficient-plane
//! index ([`RangeIndex::from_chunk_prefix`], one small ranged fetch) or from
//! a bare codestream's `TLM`/`PLT` ([`RangeIndex::from_codestream`]). Every
//! plan is a coalesced list of contiguous ranges — a standard 2D thumbnail
//! is 1–3 ranges — that any HTTP client executes with `curl -r`; decode the
//! fetched prefix bytes with
//! [`Codestream::parse_prefix`](crate::reader::Codestream::parse_prefix) +
//! [`decode_to_resolution`](crate::reader::Codestream::decode_to_resolution).

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use ndic_core::{Error, Result};
use ndic_htj2k::dwt;
use ndic_lift::AxisTransform;

use crate::container::{ChunkHeader, PlaneEntry};
use crate::reader::Codestream;

/// One contiguous byte range, **end-inclusive** — the HTTP `Range:` /
/// `curl -r` convention, so serialized plans paste straight into requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// First byte of the range.
    pub start: u64,
    /// Last byte of the range (inclusive).
    pub end: u64,
}

impl ByteRange {
    /// Bytes the range covers.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }

    /// Ranges are never empty; provided for clippy's `len` convention.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    fn of(start: u64, len: u64) -> Option<Self> {
        (len > 0).then(|| Self {
            start,
            end: start + len - 1,
        })
    }
}

/// A byte-range fetch plan: what to request and what the bytes decode to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The plan target: `thumbnail`, `thumbnail_3d`, `plane`, or `region`.
    pub target: String,
    /// Shape of the decoded result, outermost first (`[y, x]` for 2D
    /// targets, `[planes…, y, x]` for `thumbnail_3d`).
    pub decoded_size: Vec<u64>,
    /// Resolutions `0..=max_res` are covered — decode the fetched plane
    /// bytes with `decode_to_resolution(max_res)`.
    pub max_res: u8,
    /// The chunk plane indices the plan fetches, in fetch order (empty for
    /// bare-codestream targets).
    pub planes: Vec<usize>,
    /// The coalesced ranges to fetch, ascending.
    pub ranges: Vec<ByteRange>,
    /// Total bytes fetched (the sum over `ranges`).
    pub total_bytes: u64,
}

impl Plan {
    /// The same plan with every range shifted `delta` bytes — for
    /// codestreams embedded in a `.jph`/`.jp2` box or any other container.
    #[must_use]
    pub fn shifted(mut self, delta: u64) -> Self {
        for r in &mut self.ranges {
            r.start += delta;
            r.end += delta;
        }
        self
    }
}

fn plan(
    target: &str,
    decoded_size: Vec<u64>,
    max_res: u8,
    planes: Vec<usize>,
    ranges: Vec<ByteRange>,
) -> Plan {
    let ranges = coalesce(ranges);
    let total_bytes = ranges.iter().map(ByteRange::len).sum();
    Plan {
        target: target.into(),
        decoded_size,
        max_res,
        planes,
        ranges,
        total_bytes,
    }
}

/// Sort and merge adjacent/overlapping ranges.
fn coalesce(mut ranges: Vec<ByteRange>) -> Vec<ByteRange> {
    ranges.sort_unstable_by_key(|r| r.start);
    let mut out: Vec<ByteRange> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end + 1 => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

fn err_unsupported(message: &str) -> Error {
    Error::Unsupported {
        message: message.into(),
    }
}

/// Byte-range planner over an `htj2k` chunk (or a single bare codestream).
#[derive(Debug, Clone)]
pub struct RangeIndex {
    /// Chunk shape, transformed order; `[y, x]` for a bare codestream.
    dims: Vec<u32>,
    xy_levels: u8,
    /// Bytes of `[header | index]` before the first codestream (0 for a
    /// bare codestream) — every chunk plan fetches it so the executor can
    /// re-derive the index without a second round trip.
    header_len: u64,
    planes: Vec<PlaneEntry>,
}

impl RangeIndex {
    /// Builds the planner from an `htj2k` chunk prefix of at least
    /// [`ChunkHeader::required_len`] bytes.
    ///
    /// # Errors
    /// [`Error::Codestream`] on malformed headers; [`Error::Unsupported`]
    /// when the chunk was written with `index: false`.
    pub fn from_chunk_prefix(prefix: &[u8]) -> Result<Self> {
        let header = ChunkHeader::parse(prefix)?;
        let header_len = header.header_len() as u64;
        let Some(planes) = header.planes else {
            return Err(err_unsupported(
                "chunk was written without a coefficient-plane index (index: false)",
            ));
        };
        Ok(Self {
            dims: header.dims,
            xy_levels: header.xy_levels,
            header_len,
            planes,
        })
    }

    /// Builds the planner for one bare codestream from its `TLM`/`PLT`
    /// packet index — a parsed header prefix suffices
    /// ([`Codestream::parse_prefix`]).
    ///
    /// # Errors
    /// [`Error::Unsupported`] when the stream carries no `PLT` index or
    /// exceeds the 4 GiB plane bound; [`Error::Codestream`] when the index
    /// is inconsistent.
    pub fn from_codestream(cs: &Codestream<'_>) -> Result<Self> {
        Ok(Self {
            dims: alloc::vec![cs.siz.ysiz, cs.siz.xsiz],
            xy_levels: cs.cod.decomps,
            header_len: 0,
            planes: alloc::vec![PlaneEntry::from_codestream(cs, 0)?],
        })
    }

    /// Number of trailing 2D planes.
    #[must_use]
    pub fn num_planes(&self) -> usize {
        self.planes.len()
    }

    /// The chunk shape the planner describes (`[y, x]` for a bare
    /// codestream).
    #[must_use]
    pub fn dims(&self) -> &[u32] {
        &self.dims
    }

    fn plane_width(&self) -> usize {
        self.dims[self.dims.len() - 1] as usize
    }

    fn plane_height(&self) -> usize {
        self.dims[self.dims.len() - 2] as usize
    }

    /// The largest resolution whose decoded long side fits `max_px`
    /// (resolution 0 when even that exceeds it), and its decoded size.
    fn resolution_for(&self, max_px: usize) -> (u8, usize, usize) {
        let (w, h) = (self.plane_width(), self.plane_height());
        let mut best = (0u8, dwt::level_dims(w, h, self.xy_levels));
        for r in 0..=self.xy_levels {
            let dims = dwt::level_dims(w, h, self.xy_levels - r);
            if dims.0.max(dims.1) <= max_px {
                best = (r, dims);
            }
        }
        (best.0, best.1.0, best.1.1)
    }

    /// The `[header | index]` range every chunk plan leads with.
    fn header_range(&self) -> Option<ByteRange> {
        ByteRange::of(0, self.header_len)
    }

    fn plane_prefix_range(&self, plane: usize, max_res: u8) -> Result<ByteRange> {
        let p = self
            .planes
            .get(plane)
            .ok_or_else(|| Error::InvalidArgument {
                message: alloc::format!(
                    "plane {plane} out of range for a {}-plane chunk",
                    self.planes.len()
                ),
            })?;
        let len = u64::from(p.prefix[usize::from(max_res.min(self.xy_levels))]);
        ByteRange::of(p.offset, len).ok_or_else(|| Error::InvalidArgument {
            message: "plane declares an empty resolution prefix".into(),
        })
    }

    /// A 2D thumbnail: the low-resolution prefix of the chunk's first plane
    /// (the low-pass-most plane under `nd_lift`), longest side ≤ `max_px`.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] on an empty chunk.
    pub fn thumbnail(&self, max_px: usize) -> Result<Plan> {
        let (max_res, w, h) = self.resolution_for(max_px);
        let mut ranges: Vec<ByteRange> = self.header_range().into_iter().collect();
        ranges.push(self.plane_prefix_range(0, max_res)?);
        Ok(plan(
            "thumbnail",
            alloc::vec![h as u64, w as u64],
            max_res,
            alloc::vec![0],
            ranges,
        ))
    }

    /// One full plane by chunk plane index (`plane --z K`).
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] when `z` is out of range.
    pub fn plane(&self, z: usize) -> Result<Plan> {
        let p = self.planes.get(z).ok_or_else(|| Error::InvalidArgument {
            message: alloc::format!("plane {z} out of range for a {}-plane chunk", self.planes.len()),
        })?;
        let mut ranges: Vec<ByteRange> = self.header_range().into_iter().collect();
        ranges.extend(ByteRange::of(p.offset, u64::from(p.len)));
        Ok(plan(
            "plane",
            alloc::vec![self.plane_height() as u64, self.plane_width() as u64],
            self.xy_levels,
            alloc::vec![z],
            ranges,
        ))
    }

    /// A 3D preview: the low-resolution prefixes of each group's low-pass
    /// plane(s) only — downsampled in x, y, *and* the decorrelated leading
    /// axes, from a handful of ranges.
    ///
    /// `steps` are the `nd_lift` transforms the chunk was encoded under
    /// (empty when it was not decorrelated: every plane is then its own
    /// low-pass, and the plan fetches all of them).
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] when a step's dimension is not a leading
    /// chunk axis.
    pub fn thumbnail_3d(&self, max_px: usize, steps: &[AxisTransform]) -> Result<Plan> {
        let leading = &self.dims[..self.dims.len() - 2];
        for step in steps {
            if step.dimension >= leading.len() {
                return Err(Error::InvalidArgument {
                    message: alloc::format!(
                        "nd_lift step on dimension {} is not a leading chunk axis \
                         (chunk has {} leading dimensions)",
                        step.dimension,
                        leading.len()
                    ),
                });
            }
        }
        // Per leading axis: which indices hold low-pass planes.
        let masks: Vec<Vec<bool>> = leading
            .iter()
            .enumerate()
            .map(|(d, &extent)| {
                let extent = extent as usize;
                let mut mask = alloc::vec![false; extent];
                match steps.iter().find(|s| s.dimension == d) {
                    Some(step) => {
                        for i in ndic_lift::low_pass_indices(step, extent) {
                            mask[i] = true;
                        }
                    }
                    None => mask.fill(true),
                }
                mask
            })
            .collect();

        let (max_res, w, h) = self.resolution_for(max_px);
        let mut ranges: Vec<ByteRange> = self.header_range().into_iter().collect();
        let mut selected = Vec::new();
        for lin in 0..self.planes.len() {
            let mut rem = lin;
            let keep = masks.iter().rev().all(|mask| {
                let i = rem % mask.len();
                rem /= mask.len();
                mask[i]
            });
            if keep {
                ranges.push(self.plane_prefix_range(lin, max_res)?);
                selected.push(lin);
            }
        }
        let mut decoded_size: Vec<u64> = masks
            .iter()
            .map(|m| m.iter().filter(|&&b| b).count() as u64)
            .collect();
        decoded_size.push(h as u64);
        decoded_size.push(w as u64);
        Ok(plan("thumbnail_3d", decoded_size, max_res, selected, ranges))
    }

    /// A precinct-aligned sub-region plan over one parsed codestream:
    /// resolutions `0..=level`, precincts intersecting `rect` only.
    ///
    /// `rect` is `(x, y, width, height)` in **full-resolution** pixels.
    /// With the writer's default maximal precincts every resolution is one
    /// whole-plane precinct, so the plan degrades to the resolution prefix;
    /// explicit precincts (e.g. 256×256) are what keep it tight — see the
    /// precinct guidance in `docs/architecture/range-access.md`.
    ///
    /// # Errors
    /// See [`Codestream::packet_index`]; [`Error::InvalidArgument`] for an
    /// empty `rect`.
    pub fn region(cs: &Codestream<'_>, rect: (u32, u32, u32, u32), level: u8) -> Result<Plan> {
        let (rx, ry, rw, rh) = rect;
        if rw == 0 || rh == 0 {
            return Err(Error::InvalidArgument {
                message: "empty region rectangle".into(),
            });
        }
        let levels = cs.cod.decomps;
        let max_res = level.min(levels);
        let spans = cs.packet_index()?;

        // Main header + every tile-part header.
        let mut ranges: Vec<ByteRange> =
            ByteRange::of(0, cs.first_tile_offset as u64).into_iter().collect();
        for tp in &cs.tile_parts {
            ranges.extend(ByteRange::of(
                tp.offset as u64,
                (tp.body.start - tp.offset) as u64,
            ));
        }

        for span in &spans {
            if span.res > max_res {
                continue;
            }
            // The precinct grid of this resolution, in resolution pixels.
            let shift = usize::from(levels - span.res);
            let (pex, pey) = cs.cod.precinct_exp(span.res);
            let (res_w, res_h) =
                dwt::level_dims(cs.siz.xsiz as usize, cs.siz.ysiz as usize, levels - span.res);
            // rect mapped into this resolution (floor start, ceil end).
            let x0 = (rx as usize >> shift).min(res_w);
            let y0 = (ry as usize >> shift).min(res_h);
            let x1 = ((rx + rw - 1) as usize >> shift).min(res_w.saturating_sub(1));
            let y1 = ((ry + rh - 1) as usize >> shift).min(res_h.saturating_sub(1));
            let (px, py) = span.precinct;
            let pw = 1usize << pex.min(31);
            let ph = 1usize << pey.min(31);
            let hit = px * pw <= x1 && (px + 1) * pw > x0 && py * ph <= y1 && (py + 1) * ph > y0;
            if hit {
                ranges.extend(ByteRange::of(span.offset as u64, span.len as u64));
            }
        }

        let shift = usize::from(levels - max_res);
        let x0 = rx as usize >> shift;
        let y0 = ry as usize >> shift;
        let x1 = ((rx + rw - 1) as usize >> shift) + 1;
        let y1 = ((ry + rh - 1) as usize >> shift) + 1;
        Ok(plan(
            "region",
            alloc::vec![(y1 - y0) as u64, (x1 - x0) as u64],
            max_res,
            Vec::new(),
            ranges,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescing_merges_adjacent_and_overlapping() {
        let merged = coalesce(alloc::vec![
            ByteRange { start: 10, end: 19 },
            ByteRange { start: 0, end: 9 },
            ByteRange { start: 30, end: 40 },
            ByteRange { start: 35, end: 38 },
        ]);
        assert_eq!(
            merged,
            alloc::vec![
                ByteRange { start: 0, end: 19 },
                ByteRange { start: 30, end: 40 },
            ]
        );
    }

    fn chunk_index() -> RangeIndex {
        // 4 planes of 64×64, 2 levels; header 116 bytes; contiguous planes.
        RangeIndex {
            dims: alloc::vec![4, 64, 64],
            xy_levels: 2,
            header_len: 116,
            planes: (0u64..4)
                .map(|i| PlaneEntry {
                    offset: 116 + 1000 * i,
                    len: 1000,
                    prefix: alloc::vec![100, 400, 1000],
                })
                .collect(),
        }
    }

    #[test]
    fn thumbnail_is_few_ranges_and_small() {
        let plan = chunk_index().thumbnail(16).unwrap();
        assert_eq!(plan.max_res, 0);
        assert_eq!(plan.decoded_size, alloc::vec![16, 16]);
        // Header (0..116) and plane 0's R0 prefix (116..216) coalesce.
        assert_eq!(plan.ranges, alloc::vec![ByteRange { start: 0, end: 215 }]);
        assert_eq!(plan.total_bytes, 216);
    }

    #[test]
    fn thumbnail_prefers_the_largest_fitting_resolution() {
        let plan = chunk_index().thumbnail(64).unwrap();
        assert_eq!(plan.max_res, 2);
        assert_eq!(plan.decoded_size, alloc::vec![64, 64]);
        let plan = chunk_index().thumbnail(32).unwrap();
        assert_eq!(plan.max_res, 1);
        // A pixel budget below even R0 still plans R0.
        let plan = chunk_index().thumbnail(1).unwrap();
        assert_eq!(plan.max_res, 0);
        assert_eq!(plan.decoded_size, alloc::vec![16, 16]);
    }

    #[test]
    fn plane_plan_fetches_index_and_one_plane() {
        let plan = chunk_index().plane(2).unwrap();
        assert_eq!(plan.planes, alloc::vec![2]);
        assert_eq!(
            plan.ranges,
            alloc::vec![
                ByteRange { start: 0, end: 115 },
                ByteRange { start: 2116, end: 3115 },
            ]
        );
        assert!(chunk_index().plane(4).is_err());
    }

    #[test]
    fn thumbnail_3d_selects_low_pass_planes_only() {
        let step = AxisTransform {
            axis: "z".into(),
            dimension: 0,
            kind: ndic_lift::LiftKind::Lift53,
            levels: 2,
            group: 0,
        };
        let plan = chunk_index().thumbnail_3d(16, &[step]).unwrap();
        // 4 planes, 2 levels → the single ⌈4/4⌉ = 1 low-pass plane.
        assert_eq!(plan.planes, alloc::vec![0]);
        assert_eq!(plan.decoded_size, alloc::vec![1, 16, 16]);
        // Without decorrelation every plane participates.
        let plan = chunk_index().thumbnail_3d(16, &[]).unwrap();
        assert_eq!(plan.planes, alloc::vec![0, 1, 2, 3]);
        assert_eq!(plan.decoded_size, alloc::vec![4, 16, 16]);
        assert_eq!(plan.ranges.len(), 4, "prefixes of contiguous planes stay separate");
    }
}
