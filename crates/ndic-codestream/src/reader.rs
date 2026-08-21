//! Codestream reader: a pull parser over an in-memory codestream with a
//! `TLM`/`PLT`-driven packet index and partial (by-resolution) decode.
//!
//! Supported scope: single tile anchored at the canvas origin, HT blocks,
//! one quality layer, maximal precincts, reversible 5/3. Multi-component
//! streams decode independently, with the reversible colour transform
//! (RCT) inverted when `COD` signals it.

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{Error, Result};
use ndic_htj2k::{BlockPasses, dwt};

use crate::geometry::{bands_of_resolution, effective_cb};
use crate::markers::{self, Cap, Cod, Siz, Sot, parse_com, parse_plt, parse_tlm};
use crate::packet::{ParseBand, parse_packet_header};
use crate::quant::Quant;

fn err(offset: usize, message: &str) -> Error {
    Error::Codestream {
        offset,
        message: message.into(),
    }
}

/// The payload of the marker segment at `pos` whose `Lmar` is `len`, paired
/// with the offset of the byte just past the segment.
///
/// Both come out of one slicing operation. The payload is taken from `data`
/// once, and the position the caller resumes at is read back off *that
/// slice* with [`slice::subslice_range`] instead of being recomputed as
/// `pos + 2 + len`: a cursor derived from the bytes that were actually
/// parsed cannot drift from them the way a second, independent expression
/// can. `None` covers exactly the two malformed shapes the callers reject —
/// an `Lmar` smaller than the two length bytes it counts itself (an
/// inverted range) and a segment running past the end of `data`.
fn segment(data: &[u8], pos: usize, len: usize) -> Option<(&[u8], usize)> {
    let payload = data.get(pos + 4..pos + 2 + len)?;
    Some((payload, data.subslice_range(payload)?.end))
}

/// One tile-part located during parsing.
#[derive(Debug, Clone)]
pub struct TilePart {
    /// The `SOT` fields.
    pub sot: Sot,
    /// Offset of the `SOT` marker in the codestream.
    pub offset: usize,
    /// Packet-body byte range within the codestream.
    pub body: core::ops::Range<usize>,
    /// Packet lengths from `PLT` segments, in order (empty if absent).
    pub plt: Vec<u32>,
}

/// A parsed codestream: main-header parameters plus located tile-parts.
#[derive(Debug)]
pub struct Codestream<'a> {
    data: &'a [u8],
    /// Bytes this codestream spans from its first byte (`SOC`) through `EOC`.
    /// For a [`Codestream::parse_prefix`] of a truncated stream this is the
    /// *declared* extent (from `Psot`), which may exceed the bytes on hand.
    total_len: usize,
    /// The `SIZ` marker.
    pub siz: Siz,
    /// The `COD` marker.
    pub cod: Cod,
    /// The `QCD` parameters.
    pub quant: Quant,
    /// The `CAP` marker, when present.
    pub cap: Option<Cap>,
    /// Comment text from `COM`, when present.
    pub comment: Option<alloc::string::String>,
    /// Tile-part lengths from `TLM` (empty if absent).
    pub tlm: Vec<u32>,
    /// Offset of the first `SOT` (end of the main header).
    pub first_tile_offset: usize,
    /// Tile-parts in codestream order.
    pub tile_parts: Vec<TilePart>,
}

/// One packet's byte span, from the `TLM`/`PLT` index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketSpan {
    /// Component of the packet.
    pub comp: usize,
    /// Resolution of the packet.
    pub res: u8,
    /// Precinct grid position `(px, py)` within the resolution.
    pub precinct: (usize, usize),
    /// Byte offset within the codestream.
    pub offset: usize,
    /// Packet length (header + bodies).
    pub len: usize,
}

/// A fully or partially decoded image.
#[derive(Debug, Clone)]
pub struct Decoded {
    /// Decoded width (of the requested resolution).
    pub width: usize,
    /// Decoded height.
    pub height: usize,
    /// One plane per component.
    pub comps: Vec<Vec<i32>>,
}

impl<'a> Codestream<'a> {
    /// Parses the main header and locates every tile-part.
    ///
    /// # Errors
    /// [`Error::Codestream`] on malformed streams, [`Error::Unsupported`]
    /// for syntax outside the reader's supported scope.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        Self::parse_impl(data, true)
    }

    /// Parses a codestream **prefix**: a truncated stream whose main header
    /// and tile-part headers are intact but whose packet bodies (and `EOC`)
    /// may be cut short — the shape a byte-range plan fetches.
    ///
    /// Tile-part [`TilePart::body`] ranges keep their *declared* extent (from
    /// `Psot`), so [`Codestream::packet_index`] still reconstructs the full
    /// packet map from a header-only prefix; decode clamps reads to the bytes
    /// on hand and [`Codestream::decode_to_resolution`] stops at the last
    /// resolution the prefix covers.
    ///
    /// # Errors
    /// [`Error::Codestream`] when even the retained headers are malformed.
    pub fn parse_prefix(data: &'a [u8]) -> Result<Self> {
        Self::parse_impl(data, false)
    }

    #[allow(clippy::too_many_lines)]
    fn parse_impl(data: &'a [u8], strict: bool) -> Result<Self> {
        let rd16 = |pos: usize| -> Result<u16> {
            data.get(pos..pos + 2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .ok_or_else(|| err(pos, "unexpected end of codestream"))
        };
        if rd16(0)? != markers::SOC {
            return Err(err(0, "missing SOC"));
        }
        let mut pos = 2;
        let mut siz = None;
        let mut cod = None;
        let mut quant = None;
        let mut cap = None;
        let mut comment = None;
        let mut tlm = Vec::new();

        // ---- main header ----------------------------------------------
        loop {
            let marker = rd16(pos)?;
            if marker == markers::SOT {
                break;
            }
            if marker < 0xFF00 {
                return Err(err(pos, "expected a marker in the main header"));
            }
            let len = usize::from(rd16(pos + 2)?);
            let Some((payload, next)) = segment(data, pos, len) else {
                return Err(err(pos, "marker segment length out of bounds"));
            };
            match marker {
                markers::SIZ => siz = Some(Siz::parse(payload, pos)?),
                markers::COD => cod = Some(Cod::parse(payload, pos)?),
                markers::QCD => quant = Some(Quant::parse(payload, pos)?),
                markers::CAP => cap = Some(Cap::parse(payload, pos)?),
                markers::COM => comment = Some(parse_com(payload)),
                markers::TLM => {
                    for (_, l) in parse_tlm(payload, pos)? {
                        tlm.push(l);
                    }
                }
                markers::COC | markers::QCC => {
                    return Err(Error::Unsupported {
                        message: "per-component COC/QCC overrides".into(),
                    });
                }
                _ => {} // skip unknown segments (PPM, CRG, ...)
            }
            pos = next;
        }

        let siz = siz.ok_or_else(|| err(pos, "missing SIZ"))?;
        let cod = cod.ok_or_else(|| err(pos, "missing COD"))?;
        let quant = quant.ok_or_else(|| err(pos, "missing QCD"))?;
        let first_tile_offset = pos;

        // ---- tile-parts ------------------------------------------------
        let mut tile_parts: Vec<TilePart> = Vec::new();
        let total_len;
        // The declared full-stream extent from the tile-parts on hand:
        // `Psot`-carrying tile-parts end at a known byte and the last one is
        // followed by `EOC`; an open-ended (`Psot == 0`) tile-part in a
        // prefix declares nothing beyond the bytes present, so its extent
        // is just what we hold (no phantom `EOC` accounting).
        let declared_len = |tile_parts: &[TilePart]| {
            tile_parts
                .iter()
                .map(|tp| {
                    if tp.sot.psot == 0 {
                        tp.body.end
                    } else {
                        tp.body.end + 2
                    }
                })
                .max()
                .unwrap_or(data.len())
        };
        loop {
            if !strict && pos >= data.len() {
                // The prefix ends inside (or exactly at the end of) a packet
                // body; the declared extent still tells the full stream size.
                total_len = declared_len(&tile_parts);
                break;
            }
            let marker = match rd16(pos) {
                Ok(marker) => marker,
                Err(_) if !strict => {
                    total_len = declared_len(&tile_parts);
                    break;
                }
                Err(e) => return Err(e),
            };
            if marker == markers::EOC {
                total_len = pos + 2;
                break;
            }
            if marker != markers::SOT {
                return Err(err(pos, "expected SOT or EOC"));
            }
            let sot_off = pos;
            let lsot = usize::from(rd16(pos + 2)?);
            if lsot != 10 {
                return Err(err(pos, "SOT with unexpected length"));
            }
            let sot_payload = data
                .get(pos + 4..pos + 12)
                .ok_or_else(|| err(pos, "truncated SOT segment"))?;
            let sot = Sot::parse(sot_payload, pos)?;
            pos += 12;

            // Tile-part header markers until SOD.
            let mut plt = Vec::new();
            loop {
                let marker = rd16(pos)?;
                if marker == markers::SOD {
                    pos += 2;
                    break;
                }
                let len = usize::from(rd16(pos + 2)?);
                let Some((payload, next)) = segment(data, pos, len) else {
                    return Err(err(pos, "tile-part marker length out of bounds"));
                };
                match marker {
                    markers::PLT => plt.extend(parse_plt(payload, pos)?),
                    markers::COD | markers::QCD | markers::COC | markers::QCC => {
                        return Err(Error::Unsupported {
                            message: "tile-part coding-style overrides".into(),
                        });
                    }
                    _ => {}
                }
                pos = next;
            }

            let body_end = if sot.psot == 0 {
                // Open-ended last tile-part: runs to EOC (which a prefix
                // may not carry).
                if strict || data.ends_with(&markers::EOC.to_be_bytes()) {
                    data.len().saturating_sub(2)
                } else {
                    data.len()
                }
            } else {
                sot_off + sot.psot as usize
            };
            if body_end < pos || (strict && body_end > data.len()) {
                return Err(err(sot_off, "Psot out of bounds"));
            }
            tile_parts.push(TilePart {
                sot,
                offset: sot_off,
                body: pos..body_end,
                plt,
            });
            pos = body_end;
        }

        Ok(Self {
            data,
            total_len,
            siz,
            cod,
            quant,
            cap,
            comment,
            tlm,
            first_tile_offset,
            tile_parts,
        })
    }

    /// Bytes the whole codestream spans, `SOC` through `EOC` inclusive.
    ///
    /// After [`Codestream::parse`] this is the consumed length — callers
    /// walking concatenated codestreams resume at the next byte. After
    /// [`Codestream::parse_prefix`] it is the *declared* full-stream length,
    /// which the bytes on hand may not reach.
    #[must_use]
    pub fn total_len(&self) -> usize {
        self.total_len
    }

    /// Validates that the stream is within the supported decoding scope.
    fn check_supported(&self) -> Result<()> {
        if self.siz.tile_grid() != (1, 1)
            || self.siz.xosiz != 0
            || self.siz.yosiz != 0
            || self.siz.xtosiz != 0
            || self.siz.ytosiz != 0
        {
            return Err(Error::Unsupported {
                message: "multi-tile or offset-canvas codestreams".into(),
            });
        }
        if self.siz.comps.iter().any(|c| c.xr != 1 || c.yr != 1) {
            return Err(Error::Unsupported {
                message: "component subsampling".into(),
            });
        }
        if !self.cod.is_ht() {
            return Err(Error::Unsupported {
                message: "non-HT (legacy J2K-1) block coding".into(),
            });
        }
        if self.cod.wavelet != 1 || self.quant.style != 0 {
            return Err(Error::Unsupported {
                message: "irreversible (9/7) codestreams".into(),
            });
        }
        if self.cod.layers != 1 {
            return Err(Error::Unsupported {
                message: "multiple quality layers".into(),
            });
        }
        if matches!(self.cod.progression, 3 | 4) && self.cod.scod & 1 != 0 {
            return Err(Error::Unsupported {
                message: "PCRL/CPRL with explicit precinct sizes".into(),
            });
        }
        Ok(())
    }

    /// Precinct grid of resolution `r`: `(npx, npy)`.
    fn precinct_grid(&self, r: u8) -> (usize, usize) {
        let levels = self.cod.decomps;
        let (rw, rh) = dwt::level_dims(self.siz.xsiz as usize, self.siz.ysiz as usize, levels - r);
        let (ppx, ppy) = self.cod.precinct_exp(r);
        if rw == 0 || rh == 0 {
            (0, 0)
        } else {
            (rw.div_ceil(1 << ppx.min(31)), rh.div_ceil(1 << ppy.min(31)))
        }
    }

    /// The packet visit order: `(comp, res, px, py)` per the progression
    /// (T.800 §B.12, single tile, one layer).
    fn packet_sequence(&self) -> Vec<(usize, u8, usize, usize)> {
        let comps = self.siz.comps.len();
        let mut order = Vec::new();
        match self.cod.progression {
            // RPCL: resolution, position raster, component.
            2 => {
                for r in 0..=self.cod.decomps {
                    let (npx, npy) = self.precinct_grid(r);
                    for py in 0..npy {
                        for px in 0..npx {
                            for c in 0..comps {
                                order.push((c, r, px, py));
                            }
                        }
                    }
                }
            }
            // PCRL/CPRL: only reachable with maximal precincts (one per
            // resolution), where they reduce to component-major order.
            3 | 4 => {
                for c in 0..comps {
                    for r in 0..=self.cod.decomps {
                        let (npx, npy) = self.precinct_grid(r);
                        for py in 0..npy {
                            for px in 0..npx {
                                order.push((c, r, px, py));
                            }
                        }
                    }
                }
            }
            // LRCP/RLCP: resolution, component, position raster.
            _ => {
                for r in 0..=self.cod.decomps {
                    for c in 0..comps {
                        let (npx, npy) = self.precinct_grid(r);
                        for py in 0..npy {
                            for px in 0..npx {
                                order.push((c, r, px, py));
                            }
                        }
                    }
                }
            }
        }
        order
    }

    /// Reconstructs the packet index purely from `TLM`/`PLT` — no packet
    /// header is decoded.
    ///
    /// # Errors
    /// [`Error::Unsupported`] when the stream carries no `TLM`/`PLT`;
    /// [`Error::Codestream`] when the index is inconsistent.
    pub fn packet_index(&self) -> Result<Vec<PacketSpan>> {
        self.check_supported()?;
        if self.tile_parts.iter().any(|tp| tp.plt.is_empty()) {
            return Err(Error::Unsupported {
                message: "codestream has no PLT index".into(),
            });
        }
        // Cross-check TLM against the located tile-parts when present.
        if !self.tlm.is_empty() {
            let mut off = self.first_tile_offset;
            for (i, &len) in self.tlm.iter().enumerate() {
                let Some(tp) = self.tile_parts.get(i) else {
                    return Err(err(off, "TLM lists more tile-parts than found"));
                };
                if tp.offset != off {
                    return Err(err(off, "TLM offsets disagree with SOT positions"));
                }
                off += len as usize;
            }
        }

        let order = self.packet_sequence();
        let mut spans = Vec::with_capacity(order.len());
        let mut oi = 0;
        for tp in &self.tile_parts {
            let mut off = tp.body.start;
            for &len in &tp.plt {
                let Some(&(comp, res, px, py)) = order.get(oi) else {
                    return Err(err(off, "PLT lists more packets than expected"));
                };
                spans.push(PacketSpan {
                    comp,
                    res,
                    precinct: (px, py),
                    offset: off,
                    len: len as usize,
                });
                off += len as usize;
                oi += 1;
            }
            if off != tp.body.end {
                return Err(err(off, "PLT lengths do not cover the tile-part body"));
            }
        }
        if oi != order.len() {
            return Err(err(
                self.data.len(),
                "PLT covers fewer packets than expected",
            ));
        }
        Ok(spans)
    }

    /// Decodes the full image.
    ///
    /// # Errors
    /// See [`Codestream::decode_to_resolution`].
    pub fn decode(&self) -> Result<Decoded> {
        self.decode_to_resolution(self.cod.decomps)
    }

    /// Decodes resolutions `0..=max_res`, returning the reduced image.
    ///
    /// # Errors
    /// [`Error::Codestream`] on malformed packets or block data.
    #[allow(clippy::too_many_lines, clippy::similar_names)]
    pub fn decode_to_resolution(&self, max_res: u8) -> Result<Decoded> {
        self.check_supported()?;
        let max_res = max_res.min(self.cod.decomps);
        let levels = self.cod.decomps;
        let width = self.siz.xsiz as usize;
        let height = self.siz.ysiz as usize;
        let ncomps = self.siz.comps.len();

        // Concatenated packet bodies across tile-parts, clamped to the bytes
        // on hand (a parsed prefix declares more body than it carries).
        let mut body = Vec::new();
        for tp in &self.tile_parts {
            let start = tp.body.start.min(self.data.len());
            let end = tp.body.end.min(self.data.len());
            body.extend_from_slice(&self.data[start..end]);
        }

        let mut planes = alloc::vec![alloc::vec![0i32; width * height]; ncomps];
        let (cbw_n, cbh_n) = self.cod.cb_size();
        let uses_sop = self.cod.scod & 2 != 0;
        let uses_eph = self.cod.scod & 4 != 0;
        let stripe_causal = self.cod.stripe_causal();

        // Stop after the last packet the requested resolutions need: under
        // LRCP/RLCP/RPCL that is the last packet before any higher
        // resolution, under PCRL/CPRL the last component's. This is what
        // makes a byte-range *prefix* decodable — the packets past it need
        // not exist.
        let sequence = self.packet_sequence();
        let last_needed = sequence.iter().rposition(|&(_, res, _, _)| res <= max_res);

        let mut cursor = 0usize;
        for (i, (comp, res, ppx_i, ppy_i)) in sequence.into_iter().enumerate() {
            if last_needed.is_none_or(|last| i > last) {
                break;
            }
            if cursor > body.len() {
                return Err(err(cursor, "packet body truncated"));
            }
            let bands = bands_of_resolution(width, height, levels, res);
            let (ppx, ppy) = self.cod.precinct_exp(res);
            let (ecbw, ecbh) = effective_cb(cbw_n, cbh_n, ppx, ppy, res);
            // Per band: the precinct's aligned code-block sub-grid
            // (origin bx0/by0 and extent nx/ny in the band's global grid).
            // Cod::parse rejects zero exponents above resolution 0, so
            // the saturation here is belt-and-braces only.
            let shift = u8::from(res > 0);
            let sub_grids: Vec<(usize, usize, usize, usize)> = bands
                .iter()
                .map(|b| {
                    precinct_block_range(
                        b.w,
                        b.h,
                        ppx.saturating_sub(shift),
                        ppy.saturating_sub(shift),
                        ppx_i,
                        ppy_i,
                        ecbw,
                        ecbh,
                    )
                })
                .collect();
            let parse_bands: Vec<ParseBand> = bands
                .iter()
                .zip(&sub_grids)
                .map(|(b, &(_, _, nx, ny))| ParseBand {
                    nx,
                    ny,
                    k_max: self.quant.k_max(b.res, b.band),
                })
                .collect();

            let packet =
                parse_packet_header(&body[cursor..], &parse_bands, uses_sop, uses_eph, cursor)?;
            let mut p = cursor + packet.header_len;

            for blk in &packet.blocks {
                let band = &bands[blk.band];
                let (bx0, by0, _, _) = sub_grids[blk.band];
                let rect = band.block_rect(bx0 + blk.bx, by0 + blk.by, ecbw, ecbh);
                let seg_len = (blk.len_cleanup + blk.len_refinement) as usize;
                let Some(seg) = body.get(p..p + seg_len) else {
                    return Err(err(p, "packet body truncated"));
                };
                p += seg_len;
                if res > max_res {
                    continue;
                }

                let mut out = alloc::vec![0u32; rect.w * rect.h];
                ndic_htj2k::decode_block(
                    seg,
                    &mut out,
                    rect.w,
                    rect.h,
                    rect.w,
                    u32::from(blk.missing_msbs),
                    &BlockPasses {
                        num_passes: u32::from(blk.num_passes),
                        len_cleanup: blk.len_cleanup as usize,
                        len_refinement: blk.len_refinement as usize,
                    },
                    stripe_causal,
                )?;

                let k_max = self.quant.k_max(band.res, band.band);
                let shift = 31 - u32::from(k_max);
                let plane = &mut planes[comp];
                for y in 0..rect.h {
                    for x in 0..rect.w {
                        plane[(band.y0 + rect.y + y) * width + band.x0 + rect.x + x] =
                            ndic_htj2k::sign_magnitude_to_coeff(out[y * rect.w + x], shift);
                    }
                }
            }
            cursor = p;
        }

        // Inverse wavelet over the decoded region.
        let (out_w, out_h) = dwt::level_dims(width, height, levels - max_res);
        let mut comps_out = Vec::with_capacity(ncomps);
        for plane in &mut planes {
            let mut region = alloc::vec![0i32; out_w * out_h];
            for y in 0..out_h {
                region[y * out_w..(y + 1) * out_w]
                    .copy_from_slice(&plane[y * width..y * width + out_w]);
            }
            dwt::simd::inverse_53(&mut region, out_w, out_h, max_res)?;
            comps_out.push(region);
        }

        // Inverse RCT, then the DC level un-shift with clamping.
        if self.cod.mct == 1 && ncomps >= 3 {
            let (y_plane, rest) = comps_out.split_at_mut(1);
            let (cb_plane, cr_plane) = rest.split_at_mut(1);
            for ((y, cb), cr) in y_plane[0]
                .iter_mut()
                .zip(cb_plane[0].iter_mut())
                .zip(cr_plane[0].iter_mut())
            {
                let g = *y - ((*cb + *cr) >> 2);
                let r = *cr + g;
                let bl = *cb + g;
                *y = r;
                *cb = g;
                *cr = bl;
            }
        }
        for (ci, plane) in comps_out.iter_mut().enumerate() {
            let cs = self.siz.comps[ci.min(self.siz.comps.len() - 1)];
            if !cs.signed {
                let half = 1i32 << (cs.depth - 1);
                let max = (1i64 << cs.depth) - 1;
                for v in plane.iter_mut() {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        *v = i64::from(*v + half).clamp(0, max) as i32;
                    }
                }
            }
        }

        Ok(Decoded {
            width: out_w,
            height: out_h,
            comps: comps_out,
        })
    }
}

#[allow(clippy::too_many_arguments, clippy::similar_names)]
/// The aligned code-block sub-grid a precinct covers within a band:
/// `(bx0, by0, nx, ny)` in the band's global block grid.
///
/// `pp_log` are the precinct exponents in *band* coordinates (already
/// reduced by one for resolutions above 0); block sizes divide precinct
/// sizes, so precinct edges align with block boundaries.
fn precinct_block_range(
    bw: usize,
    bh: usize,
    ppx_log: u8,
    ppy_log: u8,
    px: usize,
    py: usize,
    cbw: usize,
    cbh: usize,
) -> (usize, usize, usize, usize) {
    if bw == 0 || bh == 0 {
        return (0, 0, 0, 0);
    }
    let ppw = 1usize << ppx_log.min(31);
    let pph = 1usize << ppy_log.min(31);
    let x0 = px * ppw;
    let y0 = py * pph;
    if x0 >= bw || y0 >= bh {
        return (0, 0, 0, 0);
    }
    let x1 = (x0 + ppw).min(bw);
    let y1 = (y0 + pph).min(bh);
    let bx0 = x0 / cbw;
    let by0 = y0 / cbh;
    (bx0, by0, x1.div_ceil(cbw) - bx0, y1.div_ceil(cbh) - by0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `segment` must answer exactly what the `pos + 2 + len` arithmetic it
    /// replaced answered — including on the shapes that arithmetic rejected
    /// with an explicit `len < 2` guard.
    #[test]
    fn segment_payload_and_cursor_match_the_declared_length() {
        let data: Vec<u8> = (0u8..32).collect();
        // `Lmar` counts its own two bytes: the payload spans
        // `[pos + 4, pos + 2 + len)` and the next marker sits at its end.
        assert_eq!(segment(&data, 4, 10), Some((&data[8..16], 16)));
        // `Lmar == 2` is an empty but well-formed payload (bare `COM`s).
        assert_eq!(segment(&data, 4, 2), Some((&data[8..8], 8)));
        // A segment ending exactly at the last byte still parses.
        assert_eq!(segment(&data, 4, 26), Some((&data[8..32], 32)));
        // One byte past the end does not.
        assert_eq!(segment(&data, 4, 27), None);
        // `Lmar` below the two bytes it counts itself.
        assert_eq!(segment(&data, 4, 1), None);
        assert_eq!(segment(&data, 4, 0), None);
    }
}
