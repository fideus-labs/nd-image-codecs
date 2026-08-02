//! `ndic-codestream` — HTJ2K codestream reader/writer and marker segments.
//!
//! Owns the JPEG 2000 **Part 1 / Part 15 only** syntax layer: `SIZ`, `COD`,
//! `QCD`, `CAP` (Part-15 HT signalling), and the `TLM`/`PLT` indexing
//! markers that make byte-range thumbnail/region fetch possible. Also the
//! `.jph` box wrapper.
//!
//! No JPEG 2000 Part 2 syntax is read or written: cross-axis (z/t/c)
//! decorrelation lives entirely outside the codestream, in the independently
//! specified `nd_lift` Zarr array-to-array codec (`ndic-lift`). Every
//! codestream this crate produces is a plain, conforming Part 1 + Part 15
//! 2D codestream that any HTJ2K decoder can read.
//!
//! - [`writer::encode_image`] produces a single-tile `.j2c` with always-on
//!   `TLM`/`PLT`;
//! - [`reader::Codestream`] parses, indexes (without decoding), and decodes
//!   fully or per-resolution.
//!
//! See `docs/architecture/codestream.md` and `docs/architecture/range-access.md`.
#![cfg_attr(not(feature = "std"), no_std)]

pub mod bitio;
pub mod geometry;
pub mod markers;
pub mod packet;
pub mod quant;
pub mod reader;
pub mod tagtree;
pub mod writer;

pub use reader::{Codestream, Decoded, PacketSpan};
pub use writer::encode_image;
