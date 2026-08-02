//! Marker segments of the Part 1 / Part 15 codestream syntax (T.800 Annex A,
//! T.814 §A): constants, typed structs, and their serialize/parse routines.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use ndic_core::{Error, Result};

/// Start of codestream.
pub const SOC: u16 = 0xFF4F;
/// Image and tile size.
pub const SIZ: u16 = 0xFF51;
/// Coding style default.
pub const COD: u16 = 0xFF52;
/// Coding style component override.
pub const COC: u16 = 0xFF53;
/// Quantization default.
pub const QCD: u16 = 0xFF5C;
/// Quantization component override.
pub const QCC: u16 = 0xFF5D;
/// Extended capabilities (Part-15 HT signalling).
pub const CAP: u16 = 0xFF50;
/// Comment.
pub const COM: u16 = 0xFF64;
/// Tile-part lengths, main header.
pub const TLM: u16 = 0xFF55;
/// Packet lengths, tile-part header.
pub const PLT: u16 = 0xFF58;
/// Start of tile-part.
pub const SOT: u16 = 0xFF90;
/// Start of packet (optional, in-bitstream).
pub const SOP: u16 = 0xFF91;
/// End of packet header (optional).
pub const EPH: u16 = 0xFF92;
/// Start of data.
pub const SOD: u16 = 0xFF93;
/// End of codestream.
pub const EOC: u16 = 0xFFD9;

fn err(offset: usize, message: &str) -> Error {
    Error::Codestream {
        offset,
        message: message.into(),
    }
}

/// Appends a marker with its 16-bit length (payload length + 2).
pub fn push_segment(out: &mut Vec<u8>, marker: u16, payload: &[u8]) {
    out.extend_from_slice(&marker.to_be_bytes());
    #[allow(clippy::cast_possible_truncation)] // payloads are < 65534 by construction
    out.extend_from_slice(&((payload.len() as u16) + 2).to_be_bytes());
    out.extend_from_slice(payload);
}

/// One component's signal properties in `SIZ`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentSiz {
    /// Bit depth (1..=38).
    pub depth: u8,
    /// True for signed samples.
    pub signed: bool,
    /// Horizontal subsampling.
    pub xr: u8,
    /// Vertical subsampling.
    pub yr: u8,
}

/// The `SIZ` marker: canvas, tile grid and component list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Siz {
    /// Capability field; `0x4000` marks an HTJ2K (Part 15) codestream.
    pub rsiz: u16,
    /// Canvas width.
    pub xsiz: u32,
    /// Canvas height.
    pub ysiz: u32,
    /// Canvas origin x.
    pub xosiz: u32,
    /// Canvas origin y.
    pub yosiz: u32,
    /// Tile width.
    pub xtsiz: u32,
    /// Tile height.
    pub ytsiz: u32,
    /// Tile grid origin x.
    pub xtosiz: u32,
    /// Tile grid origin y.
    pub ytosiz: u32,
    /// Components.
    pub comps: Vec<ComponentSiz>,
}

impl Siz {
    /// Serializes the payload (after `Lsiz`).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(36 + 3 * self.comps.len());
        p.extend_from_slice(&self.rsiz.to_be_bytes());
        for v in [
            self.xsiz,
            self.ysiz,
            self.xosiz,
            self.yosiz,
            self.xtsiz,
            self.ytsiz,
            self.xtosiz,
            self.ytosiz,
        ] {
            p.extend_from_slice(&v.to_be_bytes());
        }
        #[allow(clippy::cast_possible_truncation)]
        p.extend_from_slice(&(self.comps.len() as u16).to_be_bytes());
        for c in &self.comps {
            p.push((c.depth - 1) | if c.signed { 0x80 } else { 0 });
            p.push(c.xr);
            p.push(c.yr);
        }
        p
    }

    /// Parses the payload (after `Lsiz`).
    ///
    /// # Errors
    /// [`Error::Codestream`] on malformed or unsupported geometry.
    pub fn parse(p: &[u8], offset: usize) -> Result<Self> {
        if p.len() < 36 {
            return Err(err(offset, "SIZ too short"));
        }
        let rd32 = |i: usize| u32::from_be_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]);
        let csiz = usize::from(u16::from_be_bytes([p[34], p[35]]));
        if p.len() < 36 + 3 * csiz || csiz == 0 {
            return Err(err(offset, "SIZ component list truncated"));
        }
        let mut comps = Vec::with_capacity(csiz);
        for c in 0..csiz {
            let ssiz = p[36 + 3 * c];
            comps.push(ComponentSiz {
                depth: (ssiz & 0x7F) + 1,
                signed: ssiz & 0x80 != 0,
                xr: p[37 + 3 * c],
                yr: p[38 + 3 * c],
            });
        }
        Ok(Self {
            rsiz: u16::from_be_bytes([p[0], p[1]]),
            xsiz: rd32(2),
            ysiz: rd32(6),
            xosiz: rd32(10),
            yosiz: rd32(14),
            xtsiz: rd32(18),
            ytsiz: rd32(22),
            xtosiz: rd32(26),
            ytosiz: rd32(30),
            comps,
        })
    }

    /// Number of tiles horizontally and vertically.
    #[must_use]
    pub fn tile_grid(&self) -> (usize, usize) {
        let tx = (self.xsiz - self.xtosiz).div_ceil(self.xtsiz) as usize;
        let ty = (self.ysiz - self.ytosiz).div_ceil(self.ytsiz) as usize;
        (tx, ty)
    }
}

/// The `COD`/`COC` coding-style parameters we support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cod {
    /// `Scod`: bit 0 = explicit precincts, bit 1 = SOP, bit 2 = EPH.
    pub scod: u8,
    /// Progression order (0 LRCP, 1 RLCP, 2 RPCL, 3 PCRL, 4 CPRL).
    pub progression: u8,
    /// Number of quality layers.
    pub layers: u16,
    /// Multiple-component transform: 0 none, 1 RCT/ICT.
    pub mct: u8,
    /// Decomposition levels.
    pub decomps: u8,
    /// Code-block width exponent minus two.
    pub cb_width_exp: u8,
    /// Code-block height exponent minus two.
    pub cb_height_exp: u8,
    /// Code-block style; `0x40` selects the HT coder, `0x08` vertical causality.
    pub cb_style: u8,
    /// Wavelet kernel: 0 = 9/7, 1 = 5/3.
    pub wavelet: u8,
    /// Per-resolution precinct exponents `(PPx, PPy)`; empty = maximal (15,15).
    pub precincts: Vec<(u8, u8)>,
}

impl Cod {
    /// True when the block coder is HT (T.814).
    #[must_use]
    pub fn is_ht(&self) -> bool {
        self.cb_style & 0x40 != 0
    }

    /// True when packets use vertically-causal contexts.
    #[must_use]
    pub fn stripe_causal(&self) -> bool {
        self.cb_style & 0x08 != 0
    }

    /// Precinct exponents for resolution `r`.
    #[must_use]
    pub fn precinct_exp(&self, r: u8) -> (u8, u8) {
        if self.precincts.is_empty() {
            (15, 15)
        } else {
            self.precincts[usize::from(r).min(self.precincts.len() - 1)]
        }
    }

    /// Nominal code-block size.
    #[must_use]
    pub fn cb_size(&self) -> (usize, usize) {
        (1 << (self.cb_width_exp + 2), 1 << (self.cb_height_exp + 2))
    }

    /// Serializes the `COD` payload.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(10 + self.precincts.len());
        p.push(self.scod);
        p.push(self.progression);
        p.extend_from_slice(&self.layers.to_be_bytes());
        p.push(self.mct);
        p.push(self.decomps);
        p.push(self.cb_width_exp);
        p.push(self.cb_height_exp);
        p.push(self.cb_style);
        p.push(self.wavelet);
        if self.scod & 1 != 0 {
            for &(px, py) in &self.precincts {
                p.push(px | (py << 4));
            }
        }
        p
    }

    /// Parses a `COD` payload.
    ///
    /// # Errors
    /// [`Error::Codestream`] on malformed payloads.
    pub fn parse(p: &[u8], offset: usize) -> Result<Self> {
        if p.len() < 10 {
            return Err(err(offset, "COD too short"));
        }
        let scod = p[0];
        let decomps = p[5];
        let mut precincts = Vec::new();
        if scod & 1 != 0 {
            let need = usize::from(decomps) + 1;
            if p.len() < 10 + need {
                return Err(err(offset, "COD precinct list truncated"));
            }
            for i in 0..need {
                precincts.push((p[10 + i] & 0xF, p[10 + i] >> 4));
            }
        }
        Ok(Self {
            scod,
            progression: p[1],
            layers: u16::from_be_bytes([p[2], p[3]]),
            mct: p[4],
            decomps,
            cb_width_exp: p[6],
            cb_height_exp: p[7],
            cb_style: p[8],
            wavelet: p[9],
            precincts,
        })
    }
}

/// The `CAP` marker (Part 15 signalling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap {
    /// `Ccap15` (bit 5: irreversible; low 5 bits: encoded MAGB).
    pub ccap15: u16,
}

impl Cap {
    /// Builds `Ccap15` from the wavelet kind and MAGB bound, as `OpenJPH`.
    #[must_use]
    pub fn new(irreversible: bool, magb: u32) -> Self {
        let bp = if magb <= 8 {
            0
        } else if magb < 28 {
            magb - 8
        } else {
            13 + (magb >> 2)
        };
        #[allow(clippy::cast_possible_truncation)]
        Self {
            ccap15: (u16::from(irreversible) << 5) | (bp as u16 & 0x1F),
        }
    }

    /// Serializes the payload: `Pcap` with bit 15 set, then `Ccap15`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(6);
        p.extend_from_slice(&0x0002_0000u32.to_be_bytes());
        p.extend_from_slice(&self.ccap15.to_be_bytes());
        p
    }

    /// Parses the payload.
    ///
    /// # Errors
    /// [`Error::Codestream`] when `Pcap` lacks the Part-15 bit.
    pub fn parse(p: &[u8], offset: usize) -> Result<Self> {
        if p.len() < 6 {
            return Err(err(offset, "CAP too short"));
        }
        let pcap = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
        if pcap & 0x0002_0000 == 0 {
            return Err(err(offset, "CAP without the Part-15 (Pcap^15) bit"));
        }
        Ok(Self {
            ccap15: u16::from_be_bytes([p[4], p[5]]),
        })
    }
}

/// The `SOT` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sot {
    /// Tile index.
    pub isot: u16,
    /// Tile-part length from the start of `SOT` to the end of its data.
    pub psot: u32,
    /// Tile-part index within the tile.
    pub tpsot: u8,
    /// Number of tile-parts of the tile (0 = not stated here).
    pub tnsot: u8,
}

impl Sot {
    /// Serializes the payload.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(8);
        p.extend_from_slice(&self.isot.to_be_bytes());
        p.extend_from_slice(&self.psot.to_be_bytes());
        p.push(self.tpsot);
        p.push(self.tnsot);
        p
    }

    /// Parses the payload.
    ///
    /// # Errors
    /// [`Error::Codestream`] on short payloads.
    pub fn parse(p: &[u8], offset: usize) -> Result<Self> {
        if p.len() < 8 {
            return Err(err(offset, "SOT too short"));
        }
        Ok(Self {
            isot: u16::from_be_bytes([p[0], p[1]]),
            psot: u32::from_be_bytes([p[2], p[3], p[4], p[5]]),
            tpsot: p[6],
            tnsot: p[7],
        })
    }
}

/// Builds a `TLM` payload for one tile-part per tile: `Ztlm = 0`,
/// `Stlm = 0x40` (no `Ttlm`, 32-bit `Ptlm`).
#[must_use]
pub fn tlm_payload(tile_part_lengths: &[u32]) -> Vec<u8> {
    let mut p = Vec::with_capacity(2 + 4 * tile_part_lengths.len());
    p.push(0); // Ztlm
    p.push(0x40); // Stlm: ST=0, SP=1
    for &len in tile_part_lengths {
        p.extend_from_slice(&len.to_be_bytes());
    }
    p
}

/// Parses a `TLM` payload into `(Ttlm?, Ptlm)` entries.
///
/// # Errors
/// [`Error::Codestream`] on malformed field sizes.
pub fn parse_tlm(p: &[u8], offset: usize) -> Result<Vec<(Option<u16>, u32)>> {
    if p.len() < 2 {
        return Err(err(offset, "TLM too short"));
    }
    let stlm = p[1];
    let st = usize::from((stlm >> 4) & 0x3);
    let sp = if stlm & 0x40 != 0 { 4 } else { 2 };
    if st == 3 {
        return Err(err(offset, "invalid TLM ST field"));
    }
    let entry = st + sp;
    let body = &p[2..];
    if !body.len().is_multiple_of(entry) {
        return Err(err(offset, "TLM entries truncated"));
    }
    let mut out = Vec::with_capacity(body.len() / entry);
    for e in body.chunks_exact(entry) {
        let t = match st {
            1 => Some(u16::from(e[0])),
            2 => Some(u16::from_be_bytes([e[0], e[1]])),
            _ => None,
        };
        let l = if sp == 4 {
            u32::from_be_bytes([e[st], e[st + 1], e[st + 2], e[st + 3]])
        } else {
            u32::from(u16::from_be_bytes([e[st], e[st + 1]]))
        };
        out.push((t, l));
    }
    Ok(out)
}

/// Builds a `PLT` payload (`Zplt = index`) from packet lengths, using the
/// 7-bit continuation varint encoding of `Iplt`.
#[must_use]
pub fn plt_payload(zplt: u8, packet_lengths: &[u32]) -> Vec<u8> {
    let mut p = Vec::with_capacity(1 + 2 * packet_lengths.len());
    p.push(zplt);
    for &len in packet_lengths {
        let mut started = false;
        for shift in [28u32, 21, 14, 7] {
            let b = (len >> shift) & 0x7F;
            if b != 0 || started {
                #[allow(clippy::cast_possible_truncation)]
                p.push(0x80 | b as u8);
                started = true;
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        p.push((len & 0x7F) as u8);
    }
    p
}

/// Parses a `PLT` payload into packet lengths (ignoring `Zplt` continuity).
///
/// # Errors
/// [`Error::Codestream`] on a truncated varint.
pub fn parse_plt(p: &[u8], offset: usize) -> Result<Vec<u32>> {
    if p.is_empty() {
        return Err(err(offset, "PLT too short"));
    }
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut pending = false;
    for &b in &p[1..] {
        acc = (acc << 7) | u32::from(b & 0x7F);
        if b & 0x80 == 0 {
            out.push(acc);
            acc = 0;
            pending = false;
        } else {
            pending = true;
        }
    }
    if pending {
        return Err(err(offset, "PLT ends mid-varint"));
    }
    Ok(out)
}

/// Builds a `COM` payload with a Latin-1 text comment.
#[must_use]
pub fn com_payload(text: &str) -> Vec<u8> {
    let mut p = Vec::with_capacity(2 + text.len());
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(text.as_bytes());
    p
}

/// A comment read back from `COM`.
#[must_use]
pub fn parse_com(p: &[u8]) -> String {
    String::from_utf8_lossy(p.get(2..).unwrap_or_default()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siz_roundtrips_and_matches_ojph_dump() {
        let siz = Siz {
            rsiz: 0x4000,
            xsiz: 8,
            ysiz: 8,
            xosiz: 0,
            yosiz: 0,
            xtsiz: 8,
            ytsiz: 8,
            xtosiz: 0,
            ytosiz: 0,
            comps: alloc::vec![ComponentSiz {
                depth: 8,
                signed: false,
                xr: 1,
                yr: 1
            }],
        };
        let b = siz.to_bytes();
        assert_eq!(b.len() + 2, 0x29); // Lsiz from the verified dump
        assert_eq!(Siz::parse(&b, 0).unwrap(), siz);
        assert_eq!(siz.tile_grid(), (1, 1));
    }

    #[test]
    fn cod_roundtrips() {
        let cod = Cod {
            scod: 0,
            progression: 2,
            layers: 1,
            mct: 0,
            decomps: 5,
            cb_width_exp: 4,
            cb_height_exp: 4,
            cb_style: 0x40,
            wavelet: 1,
            precincts: alloc::vec![],
        };
        let b = cod.to_bytes();
        assert_eq!(Cod::parse(&b, 0).unwrap(), cod);
        assert!(cod.is_ht());
        assert_eq!(cod.precinct_exp(3), (15, 15));
        assert_eq!(cod.cb_size(), (64, 64));
    }

    #[test]
    fn cap_matches_ojph() {
        // 8-bit reversible, MAGB 7 -> Ccap15 = 0 (verified dump).
        assert_eq!(Cap::new(false, 7).ccap15, 0);
        assert_eq!(Cap::new(false, 17).ccap15, 9);
        let b = Cap::new(true, 10).to_bytes();
        assert_eq!(Cap::parse(&b, 0).unwrap().ccap15, 0x22);
    }

    #[test]
    fn tlm_and_plt_roundtrip() {
        let t = tlm_payload(&[0x1234_5678, 42]);
        let parsed = parse_tlm(&t, 0).unwrap();
        assert_eq!(parsed, alloc::vec![(None, 0x1234_5678), (None, 42)]);

        let lens = [0u32, 1, 127, 128, 300, 16_384, 2_097_152];
        let p = plt_payload(0, &lens);
        assert_eq!(parse_plt(&p, 0).unwrap(), lens);
    }
}
