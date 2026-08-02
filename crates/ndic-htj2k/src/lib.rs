//! `ndic-htj2k` — the High-Throughput JPEG 2000 block coder.
//!
//! Implements the ISO/IEC 15444-15 / ITU-T T.814 FBCOT (Fast Block Coding
//! with Optimized Truncation) coder. Unlike the Part-1 EBCOT Tier-1 MQ
//! arithmetic coder, the HT cleanup pass emits three interleaved,
//! byte-aligned sub-streams — **`MagSgn`**, **MEL** (adaptive run-length),
//! and **VLC** — that decode with no per-sample arithmetic step, which is
//! what makes HT roughly an order of magnitude faster and amenable to SIMD.
//!
//! The scalar coder is a faithful port of `OpenJPH`'s 32-bit datapath
//! (BSD-2-Clause) and is differentially tested against it:
//!
//! - [`decode_block`] decodes cleanup + optional `SigProp`/`MagRef` passes;
//! - [`encode_block`] emits a single self-contained cleanup pass (like
//!   `OpenJPH`, lossless needs nothing more).
//!
//! See `docs/architecture/ht-block-coder.md`.
#![cfg_attr(not(feature = "std"), no_std)]

pub mod block;

pub use block::{
    BlockPasses, coeff_to_sign_magnitude, decode_block, encode_block, sign_magnitude_to_coeff,
};
