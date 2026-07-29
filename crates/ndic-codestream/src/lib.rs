//! `ndic-codestream` — HTJ2K codestream reader/writer and marker segments.
//!
//! Owns the JPEG 2000 **Part 1 / Part 15 only** syntax layer: `SIZ`, `COD`,
//! `COC`, `QCD`, `CAP` (Part-15 HT signalling), and the `TLM`/`PLT` indexing
//! markers that make byte-range thumbnail/region fetch possible. Also the
//! `.jph` box wrapper.
//!
//! No JPEG 2000 Part 2 syntax is read or written: cross-axis (z/t/c)
//! decorrelation lives entirely outside the codestream, in the independently
//! specified `nd_lift` Zarr array-to-array codec (`ndic-lift`). Every
//! codestream this crate produces is a plain, conforming Part 1 + Part 15
//! 2D codestream that any HTJ2K decoder can read.
//!
//! See `docs/architecture/codestream.md` and `docs/architecture/range-access.md`.
#![cfg_attr(not(feature = "std"), no_std)]

/// Marker codes recognized by the reader/writer.
pub mod markers {
    /// Start of codestream.
    pub const SOC: u16 = 0xFF4F;
    /// Image and tile size.
    pub const SIZ: u16 = 0xFF51;
    /// Coding style default.
    pub const COD: u16 = 0xFF52;
    /// Extended capabilities (Part-15 HT signalling).
    pub const CAP: u16 = 0xFF50;
    /// Tile-part lengths, main header.
    pub const TLM: u16 = 0xFF55;
    /// Packet lengths, tile-part header.
    pub const PLT: u16 = 0xFF58;
    /// Start of data.
    pub const SOD: u16 = 0xFF93;
    /// End of codestream.
    pub const EOC: u16 = 0xFFD9;
}
