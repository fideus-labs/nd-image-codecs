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

use crate::geometry::{Band, bands_of_resolution, effective_cb};
use crate::markers::{
    self, Cap, Cod, ComponentSiz, Siz, com_payload, plt_payload, push_segment, tlm_payload,
};
use crate::packet::{BandBlocks, CodedBlock, write_packet};
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
        if comp
            .samples
            .iter()
            .any(|&v| i64::from(v) < lo || i64::from(v) > hi)
        {
            return Err(Error::InvalidArgument {
                message: "sample outside the declared dtype range".into(),
            });
        }
    }

    let levels = params.xy_levels;
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

    // ---- per-component wavelet transform + block coding ---------------
    let mut resolutions: Vec<Vec<Vec<BandBlocks>>> = Vec::new(); // [comp][res][band]
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
                band_blocks.push(code_band(&plane, width, band, ecbw, ecbh, &quant)?);
            }
            per_res.push(band_blocks);
        }
        resolutions.push(per_res);
    }

    // ---- packets in progression order ---------------------------------
    // With one layer and one precinct per resolution, every progression
    // reduces to resolution-major (LRCP/RLCP/RPCL) or component-major
    // (PCRL/CPRL) order.
    let comp_major = matches!(
        params.progression,
        ProgressionOrder::Pcrl | ProgressionOrder::Cprl
    );
    let mut packet_order = Vec::new();
    if comp_major {
        for c in 0..comps.len() {
            for r in 0..=levels {
                packet_order.push((c, r));
            }
        }
    } else {
        for r in 0..=levels {
            for c in 0..comps.len() {
                packet_order.push((c, r));
            }
        }
    }

    let mut body = Vec::new();
    let mut packet_lengths = Vec::with_capacity(packet_order.len());
    for &(c, r) in &packet_order {
        let bytes = write_packet(&resolutions[c][usize::from(r)]);
        #[allow(clippy::cast_possible_truncation)]
        packet_lengths.push(bytes.len() as u32);
        body.extend_from_slice(&bytes);
    }

    // ---- tile-part ----------------------------------------------------
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
    let psot = 12 + plt.len() + 2 + body.len(); // SOT seg + PLT + SOD + body
    let mut tile_part = Vec::with_capacity(psot);
    #[allow(clippy::cast_possible_truncation)]
    push_segment(
        &mut tile_part,
        markers::SOT,
        &markers::Sot {
            isot: 0,
            psot: psot as u32,
            tpsot: 0,
            tnsot: 1,
        }
        .to_bytes(),
    )?;
    tile_part.extend_from_slice(&plt);
    tile_part.extend_from_slice(&markers::SOD.to_be_bytes());
    tile_part.extend_from_slice(&body);

    // ---- main header --------------------------------------------------
    let w32 = u32::try_from(width).map_err(|_| Error::InvalidArgument {
        message: "width exceeds u32".into(),
    })?;
    let h32 = u32::try_from(height).map_err(|_| Error::InvalidArgument {
        message: "height exceeds u32".into(),
    })?;
    let mut out = Vec::with_capacity(tile_part.len() + 128);
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
        #[allow(clippy::cast_possible_truncation)]
        push_segment(
            &mut out,
            markers::TLM,
            &tlm_payload(&[tile_part.len() as u32]),
        )?;
    }
    out.extend_from_slice(&tile_part);
    out.extend_from_slice(&markers::EOC.to_be_bytes());
    Ok(out)
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
    for by in 0..ny {
        for bx in 0..nx {
            let r = band.block_rect(bx, by, cbw, cbh);
            let mut buf = alloc::vec![0u32; r.w * r.h];
            let mut mv = 0u32;
            for y in 0..r.h {
                for x in 0..r.w {
                    let src = plane[(band.y0 + r.y + y) * plane_width + band.x0 + r.x + x];
                    let sm = ndic_htj2k::coeff_to_sign_magnitude(src, shift);
                    mv |= sm & 0x7FFF_FFFF;
                    buf[y * r.w + x] = sm;
                }
            }
            if mv < (1 << shift) {
                blocks.push(None); // nothing significant at any coded bitplane
                continue;
            }
            let data = ndic_htj2k::encode_block(&buf, r.w, r.h, r.w, u32::from(k_max) - 1)?;
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
