//! The `htj2k` chunk container: `[header | coefficient-plane index |
//! codestreams…]`.
//!
//! A Zarr chunk compressed by the `htj2k` array-to-bytes codec holds one
//! independent Part 1/15 codestream per trailing 2D plane. This module owns
//! the byte layout that binds them together: a small fixed header describing
//! the chunk shape, then — when the codec's `index` option is on — the
//! **coefficient-plane index**: per plane, its byte range within the chunk
//! and the prefix length that covers each resolution (computed from the
//! codestream's `TLM`/`PLT`). [`crate::range::RangeIndex`] turns this into
//! byte-range plans; the header parses from a chunk *prefix* so a remote
//! reader bootstraps from one small ranged fetch.
//!
//! All integers little-endian. Layout:
//!
//! ```text
//! 0        4   magic  "ndht"
//! 4        1   version = 1, or 2 when a plane is stored
//! 5        1   flags   (bit 0: coefficient-plane index present)
//! 6        1   xy_levels
//! 7        1   ndim    (2..=32)
//! 8    4*ndim  dims, u32 each — transformed order, trailing two = (y, x)
//! ...          index: num_planes entries, each
//!                u64 offset | u32 len | (xy_levels + 1) * u32 prefix
//! ...          planes, in plane order
//! ```
//!
//! `prefix[r]` is the byte count from the plane's first byte that suffices
//! to decode resolutions `0..=r` (main header + tile-part header + the
//! leading packets) via [`crate::reader::Codestream::parse_prefix`].
//!
//! A plane is a codestream, or in version 2 a [`StoredPlane`] of raw samples when HT coding
//! would not shrink it. A stored plane's `prefix[r]` is its whole length.

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{Error, Result};
use ndic_htj2k::dwt;

use crate::reader::Decoded;

/// Magic bytes opening every `htj2k` chunk.
pub const MAGIC: [u8; 4] = *b"ndht";
/// The container version of a chunk whose planes are all codestreams.
pub const VERSION: u8 = 1;
/// The container version of a chunk holding at least one [`StoredPlane`].
pub const VERSION_STORED: u8 = 2;
/// Magic bytes opening a [`StoredPlane`]. A codestream opens with `SOC` (`FF 4F`) instead.
pub const STORED_MAGIC: [u8; 3] = *b"ndr";
/// Bit 7 of a stored plane's kind byte marks two's-complement samples.
const STORED_SIGNED: u8 = 0x80;
/// Flag bit 0: the coefficient-plane index follows the fixed header.
pub const FLAG_INDEX: u8 = 0x01;

/// Most dimensions a chunk may declare (Zarr arrays in the wild are ≤ 5-6D;
/// this bounds hostile headers).
pub const MAX_NDIM: usize = 32;
/// Most planes a chunk may declare — bounds hostile headers before the
/// entry table is sized (2^24 planes of even 1 KiB would be a 16 GiB chunk).
pub const MAX_PLANES: u64 = 1 << 24;

fn err(offset: usize, message: &str) -> Error {
    Error::Codestream {
        offset,
        message: message.into(),
    }
}

/// One plane's slot in the coefficient-plane index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneEntry {
    /// Byte offset of the plane's codestream from the chunk's first byte.
    pub offset: u64,
    /// Byte length of the plane's codestream.
    pub len: u32,
    /// `prefix[r]`: bytes from the plane's first byte that decode
    /// resolutions `0..=r`. Length `xy_levels + 1`, non-decreasing,
    /// `prefix[xy_levels] <= len`.
    pub prefix: Vec<u32>,
}

impl PlaneEntry {
    /// Builds the entry for a parsed codestream placed at `offset` within
    /// the chunk: length from the declared stream extent, per-resolution
    /// prefix lengths from the `TLM`/`PLT` packet index.
    ///
    /// # Errors
    /// See [`Codestream::packet_index`](crate::reader::Codestream::packet_index);
    /// [`Error::Unsupported`] when the stream exceeds the 4 GiB plane bound.
    pub fn from_codestream(cs: &crate::reader::Codestream<'_>, offset: u64) -> Result<Self> {
        let spans = cs.packet_index()?;
        let too_big = || Error::Unsupported {
            message: "codestream exceeds the 4 GiB plane index bound".into(),
        };
        let len = u32::try_from(cs.total_len()).map_err(|_| too_big())?;
        let prefix = (0..=cs.cod.decomps)
            .map(|r| {
                spans
                    .iter()
                    .filter(|s| s.res <= r)
                    .map(|s| u32::try_from(s.offset + s.len).map_err(|_| too_big()))
                    .try_fold(0u32, |acc, end| end.map(|e| acc.max(e)))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            offset,
            len,
            prefix,
        })
    }

    /// Builds the entry for a stored plane of `len` bytes placed at `offset`. Every resolution
    /// needs the whole plane.
    #[must_use]
    pub fn stored(offset: u64, len: u32, xy_levels: u8) -> Self {
        Self {
            offset,
            len,
            prefix: alloc::vec![len; usize::from(xy_levels) + 1],
        }
    }
}

/// A plane kept as raw samples because HT coding did not shrink it, as deflate falls back to a
/// stored block.
///
/// Layout is `"ndr" | kind | samples`. The kind byte holds the sample width in bytes (1, 2, or
/// 4) with bit 7 set for signed samples, and the samples follow in little-endian C order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredPlane<'a> {
    /// Bytes per sample: 1, 2, or 4.
    pub size: usize,
    /// Whether the samples are two's complement.
    pub signed: bool,
    /// The little-endian samples.
    pub samples: &'a [u8],
}

impl<'a> StoredPlane<'a> {
    /// Bytes before the samples.
    pub const HEADER_LEN: usize = 4;

    /// Whether plane bytes open with the stored-plane magic.
    #[must_use]
    pub fn is_stored(bytes: &[u8]) -> bool {
        bytes.starts_with(&STORED_MAGIC)
    }

    /// Total bytes of a stored plane of `pixels` samples of `size` bytes.
    #[must_use]
    pub fn stored_len(pixels: usize, size: usize) -> Option<usize> {
        pixels.checked_mul(size)?.checked_add(Self::HEADER_LEN)
    }

    /// Appends a stored plane's header. The caller appends the little-endian samples.
    ///
    /// # Panics
    /// When `size` is not 1, 2, or 4.
    pub fn write_header(out: &mut Vec<u8>, size: usize, signed: bool) {
        assert!(matches!(size, 1 | 2 | 4), "stored sample width {size}");
        out.extend_from_slice(&STORED_MAGIC);
        #[allow(clippy::cast_possible_truncation)] // asserted <= 4
        out.push(size as u8 | if signed { STORED_SIGNED } else { 0 });
    }

    /// Parses the stored plane of `pixels` samples opening `bytes`.
    ///
    /// # Errors
    /// [`Error::Codestream`] on a bad magic or sample width, or truncated samples.
    pub fn parse(bytes: &'a [u8], pixels: usize) -> Result<Self> {
        if bytes.len() < Self::HEADER_LEN || !Self::is_stored(bytes) {
            return Err(err(0, "not a stored plane"));
        }
        let kind = bytes[3];
        let size = usize::from(kind & !STORED_SIGNED);
        if !matches!(size, 1 | 2 | 4) {
            return Err(err(3, "stored plane sample width must be 1, 2, or 4 bytes"));
        }
        let samples = pixels
            .checked_mul(size)
            .and_then(|n| bytes.get(Self::HEADER_LEN..Self::HEADER_LEN.checked_add(n)?))
            .ok_or_else(|| err(bytes.len(), "truncated stored plane"))?;
        Ok(Self {
            size,
            signed: kind & STORED_SIGNED != 0,
            samples,
        })
    }

    /// Total bytes of the plane, header included.
    #[must_use]
    pub fn total_len(&self) -> usize {
        Self::HEADER_LEN + self.samples.len()
    }

    /// Widens the samples to `i32`.
    ///
    /// # Errors
    /// [`Error::Codestream`] when an unsigned 32-bit sample exceeds `i32::MAX`.
    pub fn widen(&self) -> Result<Vec<i32>> {
        let s = self.samples;
        Ok(match (self.size, self.signed) {
            (1, false) => s.iter().map(|&b| i32::from(b)).collect(),
            (1, true) => s.iter().map(|&b| i32::from(b.cast_signed())).collect(),
            (2, false) => s
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| i32::from(u16::from_le_bytes(c)))
                .collect(),
            (2, true) => s
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| i32::from(i16::from_le_bytes(c)))
                .collect(),
            (_, false) => s
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&c| i32::try_from(u32::from_le_bytes(c)))
                .collect::<core::result::Result<_, _>>()
                .map_err(|_| err(Self::HEADER_LEN, "stored uint32 sample exceeds i32"))?,
            (_, true) => s
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&c| i32::from_le_bytes(c))
                .collect(),
        })
    }

    /// Decodes resolutions `0..=max_res` of a `width x height` plane of `levels` decompositions,
    /// with the `Ssiz` depth a codestream of the same samples declares.
    ///
    /// A reduced resolution is the LL band of the forward 5/3 transform, clamped to the depth as
    /// the codestream reader clamps it. It equals the same resolution of the HT-coded plane.
    ///
    /// # Errors
    /// [`Error::Codestream`] on a geometry mismatch or an unsigned 32-bit sample beyond `i32`.
    /// [`Error::Unsupported`] when a reduced resolution exceeds the 32-bit HT datapath.
    pub fn decode_to_resolution(
        &self,
        width: usize,
        height: usize,
        levels: u8,
        max_res: u8,
    ) -> Result<(Decoded, u8)> {
        let mut samples = self.widen()?;
        if samples.len() != width * height {
            return Err(err(0, "stored plane geometry does not match the chunk"));
        }
        let depth = crate::writer::sample_depth(&samples, self.signed);
        let drop = levels - max_res.min(levels);
        if drop == 0 {
            let decoded = Decoded {
                width,
                height,
                comps: alloc::vec![samples],
            };
            return Ok((decoded, depth));
        }
        // The writer refuses the same range, so the lifting below cannot overflow.
        crate::writer::check_datapath(levels, depth)?;
        // The 5/3 lifting commutes with the DC level shift, so the unshifted samples give the
        // shifted LL band plus the shift.
        dwt::simd::forward_53(&mut samples, width, height, drop)?;
        let (out_w, out_h) = dwt::level_dims(width, height, drop);
        let mut region = Vec::with_capacity(out_w * out_h);
        for row in samples.chunks_exact(width).take(out_h) {
            region.extend_from_slice(&row[..out_w]);
        }
        if !self.signed {
            let max = i32::try_from((1i64 << depth) - 1).unwrap_or(i32::MAX);
            for v in &mut region {
                *v = (*v).clamp(0, max);
            }
        }
        let decoded = Decoded {
            width: out_w,
            height: out_h,
            comps: alloc::vec![region],
        };
        Ok((decoded, depth))
    }
}

/// The parsed (or to-be-written) chunk header and coefficient-plane index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkHeader {
    /// Chunk shape in transformed order; `dims[ndim-2]` is the plane height
    /// (y) and `dims[ndim-1]` the plane width (x).
    pub dims: Vec<u32>,
    /// In-plane decomposition levels every plane was coded with.
    pub xy_levels: u8,
    /// The coefficient-plane index, when written (`index: true`).
    pub planes: Option<Vec<PlaneEntry>>,
    /// Whether planes may be [`StoredPlane`]s, which marks the container version 2.
    pub stored: bool,
}

impl ChunkHeader {
    /// Number of trailing 2D planes: the product of the leading dims.
    #[must_use]
    pub fn num_planes(&self) -> usize {
        self.dims[..self.dims.len() - 2]
            .iter()
            .map(|&d| d as usize)
            .product()
    }

    /// Plane height (the `y` extent).
    #[must_use]
    pub fn plane_height(&self) -> u32 {
        self.dims[self.dims.len() - 2]
    }

    /// Plane width (the `x` extent).
    #[must_use]
    pub fn plane_width(&self) -> u32 {
        self.dims[self.dims.len() - 1]
    }

    /// Bytes of the fixed header for `ndim` dimensions.
    #[must_use]
    pub fn fixed_len(ndim: usize) -> usize {
        8 + 4 * ndim
    }

    /// Bytes of one index entry at `xy_levels`.
    #[must_use]
    pub fn entry_len(xy_levels: u8) -> usize {
        8 + 4 + 4 * (usize::from(xy_levels) + 1)
    }

    /// Total bytes of `[header | index]` — the offset of the first
    /// codestream when the index is present.
    #[must_use]
    pub fn header_len(&self) -> usize {
        Self::fixed_len(self.dims.len())
            + self
                .planes
                .as_ref()
                .map_or(0, |p| p.len() * Self::entry_len(self.xy_levels))
    }

    /// The stored plane opening `bytes`, when this container permits stored planes and the bytes
    /// carry the stored-plane magic. `None` means the plane is a codestream.
    ///
    /// # Errors
    /// [`Error::Codestream`] on a malformed stored plane.
    pub fn stored_plane<'a>(&self, bytes: &'a [u8]) -> Result<Option<StoredPlane<'a>>> {
        if !self.stored || !StoredPlane::is_stored(bytes) {
            return Ok(None);
        }
        let pixels = (self.plane_height() as usize)
            .checked_mul(self.plane_width() as usize)
            .ok_or_else(|| err(0, "plane area overflows"))?;
        StoredPlane::parse(bytes, pixels).map(Some)
    }

    /// Whether `bytes` opens with the container magic.
    #[must_use]
    pub fn is_container(bytes: &[u8]) -> bool {
        bytes.len() >= 4 && bytes[..4] == MAGIC
    }

    /// Total `[header | index]` length declared by a chunk prefix of at
    /// least [`ChunkHeader::fixed_len`]`(ndim)` bytes — how much a remote
    /// reader must fetch before [`ChunkHeader::parse`] succeeds.
    ///
    /// # Errors
    /// [`Error::Codestream`] when the prefix is shorter than the fixed
    /// header or the header is malformed.
    pub fn required_len(bytes: &[u8]) -> Result<usize> {
        let (ndim, xy_levels, flags, _) = Self::parse_fixed(bytes)?;
        if bytes.len() < Self::fixed_len(ndim) {
            return Err(err(bytes.len(), "chunk header prefix too short"));
        }
        let dims = Self::parse_dims(bytes, ndim)?;
        let num_planes = Self::checked_num_planes(&dims)?;
        let index_len = if flags & FLAG_INDEX == 0 {
            0
        } else {
            num_planes * Self::entry_len(xy_levels)
        };
        Ok(Self::fixed_len(ndim) + index_len)
    }

    fn parse_fixed(bytes: &[u8]) -> Result<(usize, u8, u8, bool)> {
        if bytes.len() < 8 {
            return Err(err(0, "chunk header prefix too short"));
        }
        if bytes[..4] != MAGIC {
            return Err(err(0, "not an htj2k chunk (bad magic)"));
        }
        let stored = match bytes[4] {
            VERSION => false,
            VERSION_STORED => true,
            _ => return Err(err(4, "unsupported htj2k chunk container version")),
        };
        let flags = bytes[5];
        if flags & !FLAG_INDEX != 0 {
            return Err(err(5, "unknown htj2k chunk container flags"));
        }
        let xy_levels = bytes[6];
        if xy_levels > 32 {
            return Err(err(
                6,
                "xy_levels exceeds the 32-level J2K bound (T.800 SPcod)",
            ));
        }
        let ndim = usize::from(bytes[7]);
        if !(2..=MAX_NDIM).contains(&ndim) {
            return Err(err(7, "chunk ndim outside 2..=32"));
        }
        Ok((ndim, xy_levels, flags, stored))
    }

    fn parse_dims(bytes: &[u8], ndim: usize) -> Result<Vec<u32>> {
        let mut dims = Vec::with_capacity(ndim);
        for i in 0..ndim {
            let at = 8 + 4 * i;
            let d = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
            if d == 0 {
                return Err(err(at, "zero chunk dimension"));
            }
            dims.push(d);
        }
        Ok(dims)
    }

    fn checked_num_planes(dims: &[u32]) -> Result<usize> {
        let mut n: u64 = 1;
        for &d in &dims[..dims.len() - 2] {
            n = n
                .checked_mul(u64::from(d))
                .filter(|&n| n <= MAX_PLANES)
                .ok_or_else(|| err(8, "chunk declares more planes than supported"))?;
        }
        usize::try_from(n).map_err(|_| err(8, "chunk declares more planes than supported"))
    }

    /// Serializes `[header | index]`.
    ///
    /// # Panics
    /// When `dims` exceeds [`MAX_NDIM`] dimensions or `xy_levels` exceeds
    /// the 32-level J2K bound — such a header could never
    /// [`ChunkHeader::parse`] back. Writers validate both before
    /// constructing the header (`htj2k`'s config parsing bounds the levels;
    /// its `encode_chunk` refuses over-dimensioned chunks with an error).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        assert!(
            (2..=MAX_NDIM).contains(&self.dims.len()),
            "chunk header ndim {} outside 2..={MAX_NDIM}",
            self.dims.len()
        );
        assert!(
            self.xy_levels <= 32,
            "chunk header xy_levels {} exceeds the 32-level J2K bound (T.800 SPcod)",
            self.xy_levels
        );
        let mut out = Vec::with_capacity(self.header_len());
        out.extend_from_slice(&MAGIC);
        out.push(if self.stored { VERSION_STORED } else { VERSION });
        out.push(if self.planes.is_some() { FLAG_INDEX } else { 0 });
        out.push(self.xy_levels);
        #[allow(clippy::cast_possible_truncation)] // asserted <= MAX_NDIM
        out.push(self.dims.len() as u8);
        for &d in &self.dims {
            out.extend_from_slice(&d.to_le_bytes());
        }
        if let Some(planes) = &self.planes {
            for p in planes {
                out.extend_from_slice(&p.offset.to_le_bytes());
                out.extend_from_slice(&p.len.to_le_bytes());
                for &l in &p.prefix {
                    out.extend_from_slice(&l.to_le_bytes());
                }
            }
        }
        out
    }

    /// Parses `[header | index]` from a chunk (or a chunk prefix of at
    /// least [`ChunkHeader::required_len`] bytes).
    ///
    /// # Errors
    /// [`Error::Codestream`] on truncated or malformed headers.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (ndim, xy_levels, flags, stored) = Self::parse_fixed(bytes)?;
        let fixed = Self::fixed_len(ndim);
        if bytes.len() < fixed {
            return Err(err(bytes.len(), "truncated chunk header"));
        }
        let dims = Self::parse_dims(bytes, ndim)?;
        let num_planes = Self::checked_num_planes(&dims)?;

        let planes = if flags & FLAG_INDEX == 0 {
            None
        } else {
            let entry = Self::entry_len(xy_levels);
            let need = fixed + num_planes * entry;
            if bytes.len() < need {
                return Err(err(bytes.len(), "truncated coefficient-plane index"));
            }
            let header_end = need as u64;
            let mut planes = Vec::with_capacity(num_planes);
            let mut min_offset = header_end;
            for i in 0..num_planes {
                let at = fixed + i * entry;
                let e = &bytes[at..at + entry];
                let offset = u64::from_le_bytes(e[..8].try_into().expect("8 bytes"));
                let len = u32::from_le_bytes(e[8..12].try_into().expect("4 bytes"));
                let prefix: Vec<u32> = e[12..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|&c| u32::from_le_bytes(c))
                    .collect();
                if offset < min_offset {
                    return Err(err(at, "plane offsets overlap or precede the index"));
                }
                if prefix.windows(2).any(|w| w[0] > w[1]) || prefix.last().is_some_and(|&l| l > len)
                {
                    return Err(err(at, "plane prefix lengths are inconsistent"));
                }
                min_offset = offset
                    .checked_add(u64::from(len))
                    .ok_or_else(|| err(at, "plane byte range overflows"))?;
                planes.push(PlaneEntry {
                    offset,
                    len,
                    prefix,
                });
            }
            Some(planes)
        };

        Ok(Self {
            dims,
            xy_levels,
            planes,
            stored,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChunkHeader {
        ChunkHeader {
            dims: alloc::vec![4, 16, 32],
            xy_levels: 2,
            planes: Some(
                (0u64..4)
                    .map(|i| PlaneEntry {
                        // header_len = 8 + 4*3 fixed + 4 entries of 24 = 116.
                        offset: 116 + 100 * i,
                        len: 100,
                        prefix: alloc::vec![40, 70, 100],
                    })
                    .collect(),
            ),
            stored: false,
        }
    }

    #[test]
    fn round_trips() {
        let h = sample();
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), h.header_len());
        // A fixed-header prefix is enough to learn the full index length.
        let fixed = ChunkHeader::fixed_len(3);
        assert_eq!(
            ChunkHeader::required_len(&bytes[..fixed]).unwrap(),
            bytes.len()
        );
        assert_eq!(ChunkHeader::parse(&bytes).unwrap(), h);
    }

    #[test]
    fn round_trips_without_index() {
        let h = ChunkHeader {
            planes: None,
            ..sample()
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), ChunkHeader::fixed_len(3));
        assert_eq!(ChunkHeader::parse(&bytes).unwrap(), h);
    }

    #[test]
    fn stored_flag_selects_version_two() {
        let h = ChunkHeader {
            stored: true,
            ..sample()
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes[4], VERSION_STORED);
        assert_eq!(ChunkHeader::parse(&bytes).unwrap(), h);
        assert_eq!(sample().to_bytes()[4], VERSION);
    }

    #[test]
    fn stored_planes_parse_only_in_version_two() {
        let mut plane = Vec::new();
        StoredPlane::write_header(&mut plane, 2, true);
        plane.extend((0..16 * 32).flat_map(|i: i16| (i - 256).to_le_bytes()));
        let v1 = sample();
        assert_eq!(v1.stored_plane(&plane).unwrap(), None);
        let v2 = ChunkHeader {
            stored: true,
            ..sample()
        };
        let stored = v2.stored_plane(&plane).unwrap().unwrap();
        assert_eq!(
            (stored.size, stored.signed, stored.total_len()),
            (2, true, plane.len())
        );
        assert_eq!(stored.widen().unwrap()[..3], [-256, -255, -254]);
        // Truncated samples and bad widths err.
        assert!(v2.stored_plane(&plane[..plane.len() - 1]).is_err());
        let mut bad = plane.clone();
        bad[3] = 3;
        assert!(v2.stored_plane(&bad).is_err());
        // A codestream is never mistaken for a stored plane.
        assert_eq!(v2.stored_plane(&[0xFF, 0x4F, 0xFF, 0x51]).unwrap(), None);
    }

    /// Hashed samples in `lo..=hi`, noise HT coding cannot shrink.
    fn noise(n: usize, lo: i32, hi: i32) -> Vec<i32> {
        let span = u32::try_from(i64::from(hi) - i64::from(lo) + 1).unwrap();
        (0..n)
            .map(|i| {
                let mut h = u32::try_from(i + 1).unwrap().wrapping_mul(0x9E37_79B1);
                h ^= h >> 16;
                h = h.wrapping_mul(0x85EB_CA6B);
                h ^= h >> 13;
                lo + i32::try_from(h % span).unwrap()
            })
            .collect()
    }

    #[test]
    fn stored_thumbnails_match_the_coded_plane() {
        use ndic_core::{CoeffPlane, EncodeParams};
        // Odd extents, tiny extents, and ranges that make the 5/3 low-pass overshoot the clamp.
        let cases: [(usize, usize, usize, bool, i32, i32); 6] = [
            (37, 53, 1, false, 0, 255),
            (64, 64, 1, true, -128, 127),
            (33, 17, 2, false, 0, 65535),
            (40, 24, 2, true, -32768, 32767),
            (5, 3, 4, true, -1_000_000, 1_000_000),
            (1, 9, 2, false, 0, 1023),
        ];
        for (width, height, size, signed, lo, hi) in cases {
            let samples = noise(width * height, lo, hi);
            let mut bytes = Vec::new();
            StoredPlane::write_header(&mut bytes, size, signed);
            for &v in &samples {
                bytes.extend_from_slice(&v.to_le_bytes()[..size]);
            }
            let stored = StoredPlane::parse(&bytes, width * height).unwrap();
            assert_eq!(stored.widen().unwrap(), samples);
            let depth = crate::writer::sample_depth(&samples, signed);
            let levels = 3;
            let params = EncodeParams {
                xy_levels: levels,
                ..EncodeParams::default()
            };
            let plane = CoeffPlane::new(&samples, width, height).unwrap();
            let stream =
                crate::writer::encode_image_with_depth(&[plane], depth, signed, &params).unwrap();
            let cs = crate::reader::Codestream::parse(&stream).unwrap();
            for r in 0..=levels + 1 {
                let coded = cs.decode_to_resolution(r).unwrap();
                let (synth, synth_depth) = stored
                    .decode_to_resolution(width, height, levels, r)
                    .unwrap();
                let case = (width, height, size, signed, r);
                assert_eq!(synth_depth, depth, "{case:?}");
                assert_eq!(
                    (synth.width, synth.height),
                    (coded.width, coded.height),
                    "{case:?}"
                );
                assert_eq!(synth.comps, coded.comps, "{case:?}");
            }
        }
    }

    #[test]
    fn rejects_hostile_headers() {
        let good = sample().to_bytes();
        // Bad magic.
        let mut b = good.clone();
        b[0] = b'x';
        assert!(ChunkHeader::parse(&b).is_err());
        // Future version.
        let mut b = good.clone();
        b[4] = 3;
        assert!(ChunkHeader::parse(&b).is_err());
        // Unknown flags.
        let mut b = good.clone();
        b[5] = 0x80;
        assert!(ChunkHeader::parse(&b).is_err());
        // Decomposition levels beyond the T.800 SPcod bound.
        let mut b = good.clone();
        b[6] = 33;
        assert!(ChunkHeader::parse(&b).is_err());
        // ndim outside bounds.
        let mut b = good.clone();
        b[7] = 1;
        assert!(ChunkHeader::parse(&b).is_err());
        // Zero dimension.
        let mut b = good.clone();
        b[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert!(ChunkHeader::parse(&b).is_err());
        // Overlapping plane ranges.
        let mut h = sample();
        h.planes.as_mut().unwrap()[1].offset = 150;
        assert!(ChunkHeader::parse(&h.to_bytes()).is_err());
        // Decreasing prefix lengths.
        let mut h = sample();
        h.planes.as_mut().unwrap()[0].prefix = alloc::vec![70, 40, 100];
        assert!(ChunkHeader::parse(&h.to_bytes()).is_err());
        // Prefix beyond the plane length.
        let mut h = sample();
        h.planes.as_mut().unwrap()[0].prefix = alloc::vec![40, 70, 101];
        assert!(ChunkHeader::parse(&h.to_bytes()).is_err());
        // Truncated index.
        assert!(ChunkHeader::parse(&good[..good.len() - 1]).is_err());
        // Every 1-byte truncation of the fixed header errs, never panics.
        for cut in 0..ChunkHeader::fixed_len(3) {
            assert!(ChunkHeader::parse(&good[..cut]).is_err());
        }
    }

    #[test]
    #[should_panic(expected = "32-level J2K bound")]
    fn to_bytes_refuses_over_bound_levels() {
        // `parse` rejects xy_levels > 32, so `to_bytes` must never emit it:
        // the same write/parse symmetry the ndim assertion enforces.
        let h = ChunkHeader {
            xy_levels: 33,
            ..sample()
        };
        let _ = h.to_bytes();
    }

    #[test]
    fn plane_count_overflow_is_caught() {
        let h = ChunkHeader {
            dims: alloc::vec![u32::MAX, u32::MAX, 16, 32],
            xy_levels: 2,
            planes: None,
            stored: false,
        };
        let mut bytes = h.to_bytes();
        bytes[5] = FLAG_INDEX; // pretend an index follows
        assert!(ChunkHeader::parse(&bytes).is_err());
        assert!(ChunkHeader::required_len(&bytes).is_err());
    }
}
