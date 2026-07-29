//! `ndic-htj2k` — the High-Throughput JPEG 2000 block coder.
//!
//! Implements the ISO/IEC 15444-15 FBCOT (Fast Block Coding with Optimized
//! Truncation) cleanup / `SigProp` / `MagRef` passes and their inverse. Unlike the
//! Part-1 EBCOT Tier-1 MQ arithmetic coder, the HT cleanup pass emits three
//! interleaved, byte-aligned sub-streams — **`MagSgn`**, **MEL** (adaptive
//! run-length), and **VLC** — that decode with no per-sample arithmetic step,
//! which is what makes HT roughly an order of magnitude faster and amenable to
//! SIMD.
//!
//! See `docs/architecture/ht-block-coder.md`.
#![cfg_attr(not(feature = "std"), no_std)]

use ndic_core::{CoeffPlane, Result};

/// Encode one code-block's coefficients with the HT cleanup pass. Roadmap
/// Phase 1.
///
/// # Errors
/// Returns [`ndic_core::Error::Unsupported`] until Phase 1 lands.
pub fn encode_block(_plane: &CoeffPlane<'_>) -> Result<AllocVecU8> {
    Err(ndic_core::Error::Unsupported {
        message: "encode_block: implemented in roadmap Phase 1".into(),
    })
}

#[cfg(feature = "std")]
type AllocVecU8 = std::vec::Vec<u8>;
#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(not(feature = "std"))]
type AllocVecU8 = alloc::vec::Vec<u8>;
