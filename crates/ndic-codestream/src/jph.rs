//! The `.jph` box file format — the Part 15 analogue of `.jp2`
//! (T.814 §B, reusing the T.800 Annex I box machinery).
//!
//! A minimal conforming file is
//! `[jP signature][ftyp 'jph '][jp2h [ihdr][colr]][jp2c <codestream>]`;
//! the reader walks the box list tolerantly and extracts the codestream.

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{Error, Result};

/// Box type `jP\x20\x20` (signature).
const BOX_JP: u32 = 0x6A50_2020;
/// Signature payload.
const JP_PAYLOAD: u32 = 0x0D0A_870A;
/// Box type `ftyp`.
const BOX_FTYP: u32 = 0x6674_7970;
/// Brand `jph `.
const BRAND_JPH: u32 = 0x6A70_6820;
/// Brand `jp2 `.
const BRAND_JP2: u32 = 0x6A70_3220;
/// Box type `jp2h` (header superbox).
const BOX_JP2H: u32 = 0x6A70_3268;
/// Box type `ihdr`.
const BOX_IHDR: u32 = 0x6968_6472;
/// Box type `colr`.
const BOX_COLR: u32 = 0x636F_6C72;
/// Box type `jp2c` (codestream).
const BOX_JP2C: u32 = 0x6A70_3263;

fn err(offset: usize, message: &str) -> Error {
    Error::Codestream {
        offset,
        message: message.into(),
    }
}

/// Image header fields from `ihdr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageHeader {
    /// Height.
    pub height: u32,
    /// Width.
    pub width: u32,
    /// Number of components.
    pub num_comps: u16,
    /// `BPC` field: `depth-1 | signed << 7`, or `0xFF` when per-component.
    pub bpc: u8,
}

/// A parsed `.jph`/`.jp2` file: header info plus the codestream range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JphFile {
    /// The `ihdr` fields, when a `jp2h` box was present.
    pub header: Option<ImageHeader>,
    /// Byte range of the raw codestream within the file.
    pub codestream: (usize, usize),
}

/// True when `data` starts with the JP2-family signature box.
#[must_use]
pub fn is_box_format(data: &[u8]) -> bool {
    data.len() >= 12
        && u32::from_be_bytes([data[0], data[1], data[2], data[3]]) == 12
        && u32::from_be_bytes([data[4], data[5], data[6], data[7]]) == BOX_JP
        && u32::from_be_bytes([data[8], data[9], data[10], data[11]]) == JP_PAYLOAD
}

fn rd32(data: &[u8], pos: usize) -> Result<u32> {
    data.get(pos..pos + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| err(pos, "unexpected end of box file"))
}

/// Walks the top-level box list of a `.jph`/`.jp2` file.
///
/// # Errors
/// [`Error::Codestream`] when the signature, brand, or box structure is
/// malformed, or no codestream box exists.
pub fn parse(data: &[u8]) -> Result<JphFile> {
    if !is_box_format(data) {
        return Err(err(0, "not a JP2-family box file (bad signature)"));
    }
    let mut pos = 12usize;
    let mut header = None;
    let mut codestream = None;
    let mut saw_ftyp = false;

    while pos + 8 <= data.len() {
        let lbox = rd32(data, pos)? as usize;
        let tbox = rd32(data, pos + 4)?;
        let (payload_start, payload_end) = match lbox {
            0 => (pos + 8, data.len()),
            1 => {
                let hi = u64::from(rd32(data, pos + 8)?);
                let lo = u64::from(rd32(data, pos + 12)?);
                let xl = usize::try_from((hi << 32) | lo)
                    .map_err(|_| err(pos, "XLBox exceeds addressable size"))?;
                if xl < 16 || pos + xl > data.len() {
                    return Err(err(pos, "XLBox out of bounds"));
                }
                (pos + 16, pos + xl)
            }
            2..=7 => return Err(err(pos, "invalid LBox")),
            _ => {
                if pos + lbox > data.len() {
                    return Err(err(pos, "box length out of bounds"));
                }
                (pos + 8, pos + lbox)
            }
        };

        match tbox {
            BOX_FTYP => {
                let brand = rd32(data, payload_start)?;
                let mut ok = brand == BRAND_JPH || brand == BRAND_JP2;
                let mut cp = payload_start + 8;
                while !ok && cp + 4 <= payload_end {
                    let compat = rd32(data, cp)?;
                    ok = compat == BRAND_JPH || compat == BRAND_JP2;
                    cp += 4;
                }
                if !ok {
                    return Err(err(pos, "ftyp lacks a jph/jp2 brand"));
                }
                saw_ftyp = true;
            }
            BOX_JP2H => {
                // Walk the sub-boxes for ihdr.
                let mut sp = payload_start;
                while sp + 8 <= payload_end {
                    let sl = rd32(data, sp)? as usize;
                    let st = rd32(data, sp + 4)?;
                    let send = if sl == 0 { payload_end } else { sp + sl };
                    if sl == 1 || send > payload_end || send < sp + 8 {
                        return Err(err(sp, "malformed jp2h sub-box"));
                    }
                    if st == BOX_IHDR && send - sp >= 8 + 14 {
                        header = Some(ImageHeader {
                            height: rd32(data, sp + 8)?,
                            width: rd32(data, sp + 12)?,
                            num_comps: u16::from_be_bytes([data[sp + 16], data[sp + 17]]),
                            bpc: data[sp + 18],
                        });
                    }
                    sp = send;
                }
            }
            BOX_JP2C => {
                codestream = Some((payload_start, payload_end));
                break; // the codestream is the payload we came for
            }
            _ => {}
        }
        pos = payload_end;
    }

    if !saw_ftyp {
        return Err(err(12, "missing ftyp box"));
    }
    let codestream = codestream.ok_or_else(|| err(pos, "missing jp2c codestream box"))?;
    Ok(JphFile { header, codestream })
}

/// Wraps a raw codestream in a minimal `.jph` file.
///
/// `comps` lists `(bit_depth, signed)` per component; the `colr` box signals
/// greyscale for one component and sRGB otherwise.
#[must_use]
pub fn wrap(codestream: &[u8], width: u32, height: u32, comps: &[(u8, bool)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(codestream.len() + 96);
    // Signature.
    out.extend_from_slice(&12u32.to_be_bytes());
    out.extend_from_slice(&BOX_JP.to_be_bytes());
    out.extend_from_slice(&JP_PAYLOAD.to_be_bytes());
    // ftyp: brand jph, minor 0, compat jph.
    out.extend_from_slice(&20u32.to_be_bytes());
    out.extend_from_slice(&BOX_FTYP.to_be_bytes());
    out.extend_from_slice(&BRAND_JPH.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&BRAND_JPH.to_be_bytes());
    // jp2h: ihdr(22) + colr(15).
    let same_depth = comps.windows(2).all(|w| w[0] == w[1]);
    let bpc: u8 = if same_depth && !comps.is_empty() {
        (comps[0].0 - 1) | if comps[0].1 { 0x80 } else { 0 }
    } else {
        0xFF
    };
    let bpcc_len = if bpc == 0xFF { 8 + comps.len() } else { 0 };
    #[allow(clippy::cast_possible_truncation)]
    out.extend_from_slice(&((45 + bpcc_len) as u32).to_be_bytes());
    out.extend_from_slice(&BOX_JP2H.to_be_bytes());
    out.extend_from_slice(&22u32.to_be_bytes());
    out.extend_from_slice(&BOX_IHDR.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    #[allow(clippy::cast_possible_truncation)]
    out.extend_from_slice(&(comps.len() as u16).to_be_bytes());
    out.push(bpc);
    out.push(7); // compression type: JPEG 2000
    out.push(0); // colourspace known
    out.push(0); // no IPR
    if bpc == 0xFF {
        // bpcc: per-component depths.
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&((8 + comps.len()) as u32).to_be_bytes());
        out.extend_from_slice(b"bpcc");
        for &(d, s) in comps {
            out.push((d - 1) | if s { 0x80 } else { 0 });
        }
    }
    out.extend_from_slice(&15u32.to_be_bytes());
    out.extend_from_slice(&BOX_COLR.to_be_bytes());
    out.push(1); // METH: enumerated
    out.push(0); // PREC
    out.push(0); // APPROX
    let enum_cs: u32 = if comps.len() >= 3 { 16 } else { 17 };
    out.extend_from_slice(&enum_cs.to_be_bytes());
    // jp2c with an exact length.
    #[allow(clippy::cast_possible_truncation)]
    out.extend_from_slice(&((8 + codestream.len()) as u32).to_be_bytes());
    out.extend_from_slice(&BOX_JP2C.to_be_bytes());
    out.extend_from_slice(codestream);
    out
}

/// Locates the codestream payload offset from a file **prefix**: the box
/// walk reads only box headers, so a small ranged fetch suffices and the
/// codestream box's declared extent may reach past the bytes on hand
/// (unlike [`parse`], which bounds-checks every box).
///
/// # Errors
/// [`Error::Codestream`] when the prefix is not a JP2-family file or ends
/// before the codestream box header.
pub fn codestream_offset(data: &[u8]) -> Result<usize> {
    if !is_box_format(data) {
        return Err(err(0, "not a JP2-family box file (bad signature)"));
    }
    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        let lbox = rd32(data, pos)? as usize;
        let tbox = rd32(data, pos + 4)?;
        if tbox == BOX_JP2C {
            return Ok(pos + if lbox == 1 { 16 } else { 8 });
        }
        match lbox {
            0 => break, // a non-codestream box running to EOF: nothing follows
            1 => {
                let hi = u64::from(rd32(data, pos + 8)?);
                let lo = u64::from(rd32(data, pos + 12)?);
                let xl = usize::try_from((hi << 32) | lo)
                    .map_err(|_| err(pos, "XLBox exceeds addressable size"))?;
                if xl < 16 {
                    return Err(err(pos, "XLBox too short"));
                }
                pos += xl;
            }
            2..=7 => return Err(err(pos, "invalid LBox")),
            _ => pos += lbox,
        }
    }
    Err(err(pos, "prefix ends before the jp2c codestream box"))
}

/// Returns the raw codestream: unwraps `.jph`/`.jp2` boxes, passes raw
/// `SOC`-led codestreams through.
///
/// # Errors
/// [`Error::Codestream`] when the data is neither.
pub fn unwrap_codestream(data: &[u8]) -> Result<&[u8]> {
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0x4F {
        Ok(data)
    } else if is_box_format(data) {
        let f = parse(data)?;
        Ok(&data[f.codestream.0..f.codestream.1])
    } else {
        Err(err(0, "neither a raw codestream nor a JP2-family box file"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_parse_roundtrip() {
        let fake_cs = [0xFFu8, 0x4F, 0xFF, 0xD9];
        let file = wrap(&fake_cs, 640, 480, &[(8, false); 3]);
        assert!(is_box_format(&file));
        let parsed = parse(&file).unwrap();
        let (s, e) = parsed.codestream;
        assert_eq!(&file[s..e], &fake_cs);
        let h = parsed.header.unwrap();
        assert_eq!((h.width, h.height, h.num_comps, h.bpc), (640, 480, 3, 7));
    }

    #[test]
    fn mixed_depths_use_bpcc() {
        let file = wrap(&[0u8; 2], 4, 4, &[(8, false), (12, true)]);
        let parsed = parse(&file).unwrap();
        assert_eq!(parsed.header.unwrap().bpc, 0xFF);
    }

    #[test]
    fn rejects_bad_signature() {
        assert!(parse(&[0u8; 32]).is_err());
        assert!(!is_box_format(&[0u8; 4]));
    }
}
