//! Codestream reader: a pull parser over an in-memory codestream with a
//! `TLM`/`PLT`-driven packet index and partial (by-resolution) decode.
//!
//! Phase-3 scope: single tile anchored at the canvas origin, HT blocks,
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
    /// for syntax outside the Phase-3 reader.
    #[allow(clippy::too_many_lines)]
    pub fn parse(data: &'a [u8]) -> Result<Self> {
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
            if len < 2 || pos + 2 + len > data.len() {
                return Err(err(pos, "marker segment length out of bounds"));
            }
            let payload = &data[pos + 4..pos + 2 + len];
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
            pos += 2 + len;
        }

        let siz = siz.ok_or_else(|| err(pos, "missing SIZ"))?;
        let cod = cod.ok_or_else(|| err(pos, "missing COD"))?;
        let quant = quant.ok_or_else(|| err(pos, "missing QCD"))?;
        let first_tile_offset = pos;

        // ---- tile-parts ------------------------------------------------
        let mut tile_parts = Vec::new();
        loop {
            let marker = rd16(pos)?;
            if marker == markers::EOC {
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
            let sot = Sot::parse(&data[pos + 4..pos + 12], pos)?;
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
                if len < 2 || pos + 2 + len > data.len() {
                    return Err(err(pos, "tile-part marker length out of bounds"));
                }
                let payload = &data[pos + 4..pos + 2 + len];
                match marker {
                    markers::PLT => plt.extend(parse_plt(payload, pos)?),
                    markers::COD | markers::QCD | markers::COC | markers::QCC => {
                        return Err(Error::Unsupported {
                            message: "tile-part coding-style overrides".into(),
                        });
                    }
                    _ => {}
                }
                pos += 2 + len;
            }

            let body_end = if sot.psot == 0 {
                // Open-ended last tile-part: runs to EOC.
                data.len().saturating_sub(2)
            } else {
                sot_off + sot.psot as usize
            };
            if body_end < pos || body_end > data.len() {
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

    /// Validates that the stream is within the Phase-3 decoding scope.
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

        // Concatenated packet bodies across tile-parts.
        let mut body = Vec::new();
        for tp in &self.tile_parts {
            body.extend_from_slice(&self.data[tp.body.clone()]);
        }

        let mut planes = alloc::vec![alloc::vec![0i32; width * height]; ncomps];
        let (cbw_n, cbh_n) = self.cod.cb_size();
        let uses_sop = self.cod.scod & 2 != 0;
        let uses_eph = self.cod.scod & 4 != 0;
        let stripe_causal = self.cod.stripe_causal();

        let mut cursor = 0usize;
        for (comp, res, ppx_i, ppy_i) in self.packet_sequence() {
            let bands = bands_of_resolution(width, height, levels, res);
            let (ppx, ppy) = self.cod.precinct_exp(res);
            let (ecbw, ecbh) = effective_cb(cbw_n, cbh_n, ppx, ppy, res);
            // Per band: the precinct's aligned code-block sub-grid
            // (origin bx0/by0 and extent nx/ny in the band's global grid).
            let shift = u8::from(res > 0);
            let sub_grids: Vec<(usize, usize, usize, usize)> = bands
                .iter()
                .map(|b| {
                    precinct_block_range(
                        b.w,
                        b.h,
                        ppx - shift,
                        ppy - shift,
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
            dwt::inverse_53(&mut region, out_w, out_h, max_res)?;
            comps_out.push(region);
        }

        // Inverse RCT, then the DC level un-shift with clamping.
        if self.cod.mct == 1 && ncomps >= 3 {
            let (a, rest) = comps_out.split_at_mut(1);
            let (b, c) = rest.split_at_mut(1);
            for ((y, cb), cr) in a[0].iter_mut().zip(b[0].iter_mut()).zip(c[0].iter_mut()) {
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
