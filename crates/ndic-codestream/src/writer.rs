//! Codestream writer: encodes 2D planes into a single-tile HTJ2K `.j2c`
//! codestream with always-on `TLM`/`PLT` indexing.
//!
//! The writer emits `SOC SIZ COD QCD CAP COM TLM / SOT PLT SOD <packets> EOC`
//! with RPCL progression by default, one quality layer, maximal precincts,
//! and cleanup-only HT code-blocks — the same shape `OpenJPH` produces, plus
//! the indexing markers.

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{CoeffPlane, EncodeParams, Error, ProgressionOrder, Result, SampleType};
use ndic_htj2k::dwt;

use crate::geometry::{
    Band, bands_of_resolution, effective_cb, packet_sequence, precinct_block_range, precinct_grid,
};
use crate::markers::{
    self, Cap, Cod, ComponentSiz, Siz, com_payload, plt_payload, push_segment, tlm_payload,
};
use crate::packet::{BandBlocks, CodedBlock, packet_header};
use crate::quant::Quant;

/// Encodes one or more equally-sized component planes into a codestream.
///
/// Components are coded independently (no colour transform), losslessly
/// with the 5/3 wavelet.
///
/// # Errors
/// [`Error::InvalidArgument`] on inconsistent inputs, [`Error::Unsupported`]
/// for parameter combinations the writer does not support (9/7, multiple
/// layers, custom precincts).
pub fn encode_image(
    comps: &[CoeffPlane<'_>],
    dtype: SampleType,
    params: &EncodeParams,
) -> Result<Vec<u8>> {
    encode_image_with_depth(comps, dtype.bit_depth(), dtype.is_signed(), params)
}

/// [`encode_image`] with an explicit `Ssiz` sample format instead of a
/// [`SampleType`].
///
/// JPEG 2000 declares per-component precision freely (1–38 bits), so callers
/// holding `i32` planes whose *actual* dynamic range is narrower — post
/// `nd_lift` coefficient planes above all — declare just the bits they use.
/// The 32-bit HT datapath admits declared depths up to `30 - X` bits, where
/// `X` is the 5/3 ranging gain (2–4 bits); a full 32-bit declaration is
/// rejected, values that *fit* a narrower declaration are not.
///
/// # Errors
/// See [`encode_image`]; additionally [`Error::InvalidArgument`] when a
/// sample falls outside the declared range and [`Error::Unsupported`] when
/// `bit_depth` exceeds the HT datapath.
#[allow(clippy::too_many_lines, clippy::similar_names)]
pub fn encode_image_with_depth(
    comps: &[CoeffPlane<'_>],
    bit_depth: u8,
    signed: bool,
    params: &EncodeParams,
) -> Result<Vec<u8>> {
    let Some(first) = comps.first() else {
        return Err(Error::InvalidArgument {
            message: "at least one component required".into(),
        });
    };
    let (width, height) = (first.width, first.height);
    if width == 0 || height == 0 {
        return Err(Error::InvalidArgument {
            message: "empty image".into(),
        });
    }
    if comps.iter().any(|c| c.width != width || c.height != height) {
        return Err(Error::InvalidArgument {
            message: "all components must have identical dimensions".into(),
        });
    }
    if params.wavelet != ndic_core::WaveletKind::Reversible53 {
        return Err(Error::Unsupported {
            message: "writer is lossless (5/3) only".into(),
        });
    }
    if params.quality_layers != 1 {
        return Err(Error::Unsupported {
            message: "writer emits a single quality layer".into(),
        });
    }
    if !params.precincts.is_empty() {
        return Err(Error::Unsupported {
            message: "writer uses maximal precincts".into(),
        });
    }
    let (cbw, cbh) = (
        usize::from(params.code_block.0),
        usize::from(params.code_block.1),
    );
    if !(4..=1024).contains(&cbw)
        || !(4..=1024).contains(&cbh)
        || !cbw.is_power_of_two()
        || !cbh.is_power_of_two()
        || cbw * cbh > 4096
    {
        return Err(Error::InvalidArgument {
            message: "code-block sides must be powers of two in 4..=1024, area <= 4096".into(),
        });
    }

    if !(1..=38).contains(&bit_depth) {
        return Err(Error::InvalidArgument {
            message: "declared bit depth must be in 1..=38".into(),
        });
    }
    // Validate the sample range against the declared format; out-of-range
    // values would overflow the ranging bounds.
    let (lo, hi) = if signed {
        (-(1i64 << (bit_depth - 1)), (1i64 << (bit_depth - 1)) - 1)
    } else {
        (0, (1i64 << bit_depth) - 1)
    };
    for comp in comps {
        // A min/max reduction vectorizes, where an early-exit scan loads one sample at a time.
        let (min, max) = comp
            .samples
            .iter()
            .fold((i32::MAX, i32::MIN), |(min, max), &v| {
                (min.min(v), max.max(v))
            });
        if !comp.samples.is_empty() && (i64::from(min) < lo || i64::from(max) > hi) {
            return Err(Error::InvalidArgument {
                message: "sample outside the declared dtype range".into(),
            });
        }
    }

    let levels = params.xy_levels;
    let quant = check_datapath(levels, bit_depth)?;

    // ---- per-component wavelet transform + block coding ---------------
    let mut resolutions: Vec<Vec<Vec<BandBlocks>>> = Vec::new(); // [comp][res][band]
    let mut coder = ndic_htj2k::BlockEncoder::new();
    for comp in comps {
        let mut plane = level_shifted(comp, bit_depth, signed);
        // The SIMD lane is bit-identical to the scalar reference
        // (differential-tested) and substantially faster.
        dwt::simd::forward_53(&mut plane, width, height, levels)?;
        let mut per_res = Vec::with_capacity(usize::from(levels) + 1);
        for r in 0..=levels {
            let bands = bands_of_resolution(width, height, levels, r);
            let (ecbw, ecbh) = effective_cb(cbw, cbh, 15, 15, r);
            let mut band_blocks = Vec::with_capacity(bands.len());
            for band in &bands {
                band_blocks.push(code_band(
                    &mut coder, &plane, width, band, ecbw, ecbh, &quant,
                )?);
            }
            per_res.push(band_blocks);
        }
        resolutions.push(per_res);
    }

    // ---- packets in progression order ---------------------------------
    // A maximal precinct spans 2^15 samples of its resolution, so a larger resolution splits into
    // several precincts, each a packet holding its share of every band's code-blocks.
    let packet_order = packet_sequence(
        progression_code(params.progression),
        comps.len(),
        levels,
        |r| precinct_grid(width, height, levels, r, 15, 15),
        |_| (15, 15),
    );
    // Headers come first, so the tile-part's length and its PLT are known before any block data
    // is written, and the data moves once, straight into the codestream.
    let mut packets = Vec::with_capacity(packet_order.len());
    let mut packet_lengths = Vec::with_capacity(packet_order.len());
    let mut body_len = 0usize;
    for (c, r, px, py) in packet_order {
        let (ecbw, ecbh) = effective_cb(cbw, cbh, 15, 15, r);
        // Precinct exponents in band coordinates.
        let pp = 15 - u8::from(r > 0);
        let bands = bands_of_resolution(width, height, levels, r);
        let precinct: Vec<BandBlocks> = bands
            .iter()
            .zip(&mut resolutions[c][usize::from(r)])
            .map(|(band, coded)| {
                let (bx0, by0, nx, ny) =
                    precinct_block_range(band.w, band.h, pp, pp, px, py, ecbw, ecbh);
                // Each block belongs to one precinct, so it moves out rather than being copied.
                let blocks = (by0..by0 + ny)
                    .flat_map(|by| (bx0..bx0 + nx).map(move |bx| (bx, by)))
                    .map(|(bx, by)| coded.blocks[by * coded.nx + bx].take())
                    .collect();
                BandBlocks { nx, ny, blocks }
            })
            .collect();
        let header = packet_header(&precinct);
        let len = header.len() + included(&precinct).map(|cb| cb.data.len()).sum::<usize>();
        #[allow(clippy::cast_possible_truncation)]
        packet_lengths.push(len as u32);
        body_len += len;
        packets.push((header, precinct));
    }

    // ---- tile-part header ---------------------------------------------
    let mut plt = Vec::new();
    if params.emit_tlm_plt {
        // A PLT payload is bounded by the 16-bit segment length; split the
        // packet list across as many PLT segments (Zplt 0, 1, ...) as needed.
        let mut zplt = 0u8;
        let mut start = 0usize;
        loop {
            let mut end = start;
            let mut payload_len = 1usize; // the Zplt byte
            while end < packet_lengths.len() {
                let bits = 32 - (packet_lengths[end] | 1).leading_zeros() as usize;
                let vlen = bits.div_ceil(7);
                if payload_len + vlen > 65_000 {
                    break;
                }
                payload_len += vlen;
                end += 1;
            }
            push_segment(
                &mut plt,
                markers::PLT,
                &plt_payload(zplt, &packet_lengths[start..end]),
            )?;
            start = end;
            if start >= packet_lengths.len() {
                break;
            }
            zplt = zplt.checked_add(1).ok_or_else(|| Error::InvalidArgument {
                message: "more than 256 PLT segments required".into(),
            })?;
        }
    }
    let tile_part_len = 12 + plt.len() + 2 + body_len; // SOT seg + PLT + SOD + body
    // `Psot` and the `TLM` entry are 32-bit, so a single tile-part holds at most 4 GiB.
    let psot = u32::try_from(tile_part_len).map_err(|_| Error::Unsupported {
        message: "tile-part exceeds the 4 GiB a 32-bit Psot can declare".into(),
    })?;

    // ---- main header --------------------------------------------------
    let w32 = u32::try_from(width).map_err(|_| Error::InvalidArgument {
        message: "width exceeds u32".into(),
    })?;
    let h32 = u32::try_from(height).map_err(|_| Error::InvalidArgument {
        message: "height exceeds u32".into(),
    })?;
    let mut out = Vec::with_capacity(tile_part_len + 128);
    out.extend_from_slice(&markers::SOC.to_be_bytes());
    let siz = Siz {
        rsiz: 0x4000,
        xsiz: w32,
        ysiz: h32,
        xosiz: 0,
        yosiz: 0,
        xtsiz: w32,
        ytsiz: h32,
        xtosiz: 0,
        ytosiz: 0,
        comps: comps
            .iter()
            .map(|_| ComponentSiz {
                depth: bit_depth,
                signed,
                xr: 1,
                yr: 1,
            })
            .collect(),
    };
    push_segment(&mut out, markers::SIZ, &siz.to_bytes())?;
    #[allow(clippy::cast_possible_truncation)] // cb sides are 4..=1024
    let (cbw_exp, cbh_exp) = (
        (cbw.trailing_zeros() - 2) as u8,
        (cbh.trailing_zeros() - 2) as u8,
    );
    let cod = Cod {
        scod: 0,
        progression: progression_code(params.progression),
        layers: 1,
        mct: 0,
        decomps: levels,
        cb_width_exp: cbw_exp,
        cb_height_exp: cbh_exp,
        cb_style: 0x40
            | if params.cbstyle.vertically_causal {
                0x08
            } else {
                0
            },
        wavelet: 1,
        precincts: Vec::new(),
    };
    push_segment(&mut out, markers::COD, &cod.to_bytes())?;
    push_segment(&mut out, markers::QCD, &quant.to_bytes())?;
    push_segment(
        &mut out,
        markers::CAP,
        &Cap::new(false, quant.magb()).to_bytes(),
    )?;
    push_segment(
        &mut out,
        markers::COM,
        &com_payload("nd-image-codecs ndic-codestream"),
    )?;
    if params.emit_tlm_plt {
        push_segment(&mut out, markers::TLM, &tlm_payload(&[psot]))?;
    }

    // ---- tile-part ----------------------------------------------------
    push_segment(
        &mut out,
        markers::SOT,
        &markers::Sot {
            isot: 0,
            psot,
            tpsot: 0,
            tnsot: 1,
        }
        .to_bytes(),
    )?;
    out.extend_from_slice(&plt);
    out.extend_from_slice(&markers::SOD.to_be_bytes());
    for (header, precinct) in &packets {
        out.extend_from_slice(header);
        for cb in included(precinct) {
            out.extend_from_slice(&cb.data);
        }
    }
    out.extend_from_slice(&markers::EOC.to_be_bytes());
    Ok(out)
}

/// The included code-blocks of a precinct's bands, in the order their data follows the packet
/// header.
fn included(bands: &[BandBlocks]) -> impl Iterator<Item = &CodedBlock> {
    bands.iter().flat_map(|band| band.blocks.iter().flatten())
}

/// Progression order code for `SGcod`.
fn progression_code(p: ProgressionOrder) -> u8 {
    match p {
        ProgressionOrder::Lrcp => 0,
        ProgressionOrder::Rlcp => 1,
        ProgressionOrder::Rpcl => 2,
        ProgressionOrder::Pcrl => 3,
        ProgressionOrder::Cprl => 4,
    }
}

/// The reversible quantization for `levels` decompositions of `bit_depth` samples, refused when
/// a subband would exceed the 32-bit HT datapath.
///
/// # Errors
/// [`Error::Unsupported`] when `K_max` exceeds 30.
pub(crate) fn check_datapath(levels: u8, bit_depth: u8) -> Result<Quant> {
    let quant = Quant::reversible(levels, bit_depth, false)?;
    // The scalar HT coder uses a 32-bit datapath: K_max <= 30.
    let k_max_max = (0..=levels)
        .flat_map(|r| (u8::from(r > 0)..=if r > 0 { 3 } else { 0 }).map(move |b| (r, b)))
        .map(|(r, b)| quant.k_max(r, b))
        .max()
        .unwrap_or(0);
    if k_max_max > 30 {
        return Err(Error::Unsupported {
            message: "bit depth exceeds the 32-bit HT datapath (K_max > 30)".into(),
        });
    }
    Ok(quant)
}

/// The narrowest `Ssiz` depth covering the samples' actual range.
#[must_use]
pub fn sample_depth(samples: &[i32], signed: bool) -> u8 {
    let (lo, hi) = samples
        .iter()
        .fold((0i32, 0i32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    // Both bounds are non-negative here, so a bit length is `32 - leading_zeros`. The signed
    // range also needs a sign bit and must hold `!lo = -lo - 1`, the largest magnitude below zero.
    let bit_len = |v: i32| 32 - v.leading_zeros();
    let depth = if signed {
        bit_len(hi.max(!lo)) + 1
    } else {
        bit_len(hi)
    };
    #[allow(clippy::cast_possible_truncation)] // clamped to 1..=32
    {
        depth.clamp(1, 32) as u8
    }
}

/// Applies the DC level shift and returns an owned working plane.
fn level_shifted(comp: &CoeffPlane<'_>, bit_depth: u8, signed: bool) -> Vec<i32> {
    let mut v = comp.samples.to_vec();
    if !signed {
        let half = 1i32 << (bit_depth - 1);
        for s in &mut v {
            *s -= half;
        }
    }
    v
}

/// Codes every code-block of one band from the Mallat plane.
fn code_band(
    coder: &mut ndic_htj2k::BlockEncoder,
    plane: &[i32],
    plane_width: usize,
    band: &Band,
    cbw: usize,
    cbh: usize,
    quant: &Quant,
) -> Result<BandBlocks> {
    let (nx, ny) = band.grid(cbw, cbh);
    let mut blocks = Vec::with_capacity(nx * ny);
    let k_max = quant.k_max(band.res, band.band);
    let shift = 31 - u32::from(k_max);
    // One block buffer for the whole band. The copy below writes every sample, so a reused
    // buffer needs no zeroing between blocks.
    let mut buf: Vec<u32> = Vec::new();
    for by in 0..ny {
        for bx in 0..nx {
            let r = band.block_rect(bx, by, cbw, cbh);
            buf.resize(r.w * r.h, 0);
            let mut mv = 0u32;
            for y in 0..r.h {
                // Row slices hoist the index math and bounds checks out of the sample loop.
                let src_start = (band.y0 + r.y + y) * plane_width + band.x0 + r.x;
                let src = &plane[src_start..src_start + r.w];
                let dst = &mut buf[y * r.w..(y + 1) * r.w];
                for (d, &s) in dst.iter_mut().zip(src) {
                    let sm = ndic_htj2k::coeff_to_sign_magnitude(s, shift);
                    mv |= sm & 0x7FFF_FFFF;
                    *d = sm;
                }
            }
            if mv < (1 << shift) {
                blocks.push(None); // nothing significant at any coded bitplane
                continue;
            }
            let data = coder.encode(&buf, r.w, r.h, r.w, u32::from(k_max) - 1)?;
            #[allow(clippy::cast_possible_truncation)]
            blocks.push(Some(CodedBlock {
                missing_msbs: k_max - 1,
                num_passes: 1,
                len_cleanup: data.len() as u32,
                len_refinement: 0,
                data,
            }));
        }
    }
    Ok(BandBlocks { nx, ny, blocks })
}
