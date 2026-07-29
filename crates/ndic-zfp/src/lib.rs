//! `ndic-zfp` — pure-Rust ZFP block compression (`nd_zfp`).
//!
//! A from-scratch Rust implementation of the [ZFP](https://zfp.llnl.gov)
//! fixed-block compressed-array format for **2D, 3D, and 4D** data, plus a
//! brick index, registered as the Zarr v3 **array-to-bytes** codec `nd_zfp`.
//! Target use: GPU volume rendering, random brick access, and predictable
//! memory (fixed-rate mode gives every `4^d` block a fixed bit budget, so
//! brick addresses are computable without an index lookup).
//!
//! ## Compatibility contract
//!
//! Bitstream-compatible with the reference C implementation
//! (<https://github.com/LLNL/zfp>): compressed output is byte-identical for
//! identical parameters, verified against the upstream test suite's
//! checksums (see `docs/development/roadmap/phase-5-nd-zfp.md` for the port
//! and test-reproduction plan). Until parity is proven in CI, the `zfp-sys`
//! FFI reference lane is the ground truth.
//!
//! ## Modes
//!
//! - [`ZfpMode::FixedRate`] — fixed bits per block; random access, bounded
//!   memory (primary mode for GPU bricks).
//! - [`ZfpMode::FixedAccuracy`] — absolute error bound, variable size.
//! - [`ZfpMode::FixedPrecision`] — relative-precision control.
//! - [`ZfpMode::Reversible`] — bit-for-bit lossless.
//!
//! Supported sample types: `f32`, `f64`, `i32`, `i64` (narrower integers are
//! promoted, matching the C library's advice).
#![cfg_attr(not(feature = "std"), no_std)]
#![allow(clippy::missing_const_for_fn)]

extern crate alloc;
use alloc::vec::Vec;

use ndic_core::Result;

/// The registered Zarr v3 array-to-bytes codec identifier.
pub const CODEC_NAME: &str = "nd_zfp";

/// ZFP compression mode, mirroring the reference implementation's modes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZfpMode {
    /// Fixed bits per `4^d` block (`rate` = bits per value).
    FixedRate(f64),
    /// Absolute error tolerance.
    FixedAccuracy(f64),
    /// Number of uncompressed bit planes encoded per block.
    FixedPrecision(u32),
    /// Bit-for-bit lossless.
    Reversible,
}

/// Compress a 2D/3D/4D array of `f32` samples. Roadmap Phase 5.
///
/// # Errors
/// Returns [`ndic_core::Error::Unsupported`] until Phase 5 lands.
pub fn compress_f32(_data: &[f32], _shape: &[usize], _mode: ZfpMode) -> Result<Vec<u8>> {
    Err(ndic_core::Error::Unsupported {
        message: "nd_zfp compress: implemented in roadmap Phase 5".into(),
    })
}

/// Decompress into a caller-owned buffer. Roadmap Phase 5.
///
/// # Errors
/// Returns [`ndic_core::Error::Unsupported`] until Phase 5 lands.
pub fn decompress_f32(
    _bytes: &[u8],
    _shape: &[usize],
    _mode: ZfpMode,
    _out: &mut [f32],
) -> Result<()> {
    Err(ndic_core::Error::Unsupported {
        message: "nd_zfp decompress: implemented in roadmap Phase 5".into(),
    })
}
