//! `ndic-lift` — explicit ND decorrelation transforms (`nd_lift`).
//!
//! The cross-axis half of the nd-image-codecs design: an **independently
//! specified** set of integer/float lifting transforms applied along
//! non-primary-spatial axes (z, t, and optionally c) of a Zarr chunk,
//! *before* the per-plane 2D codec runs. Registered as the Zarr v3
//! **array-to-array** codec `nd_lift`.
//!
//! Deliberately **not** JPEG 2000 Part 2: no MCT/MCC/MCO marker syntax, no
//! Part 2 capability signalling. The predictor, update rule, rounding,
//! boundary extension, coefficient ordering, and overflow semantics are all
//! defined by this codec's own specification
//! (`docs/architecture/nd-transform.md`), built from long-published lifting
//! and differencing primitives.
//!
//! Per-axis transform kinds:
//! - [`LiftKind::Delta`] — first-order differencing (fastest; longest
//!   dependency chain, bounded by the group size).
//! - [`LiftKind::Haar`] — reversible Haar lifting (fast, compact support).
//! - [`LiftKind::Lift53`] — reversible 5/3-style lifting (better smooth-signal
//!   decorrelation).
//! - 9/7-style float lifting + per-band quantization is the lossy extension
//!   (roadmap Phase 2).
//!
//! Transforms are applied within bounded **groups** along each axis (e.g.
//! 8–32 samples), capping decode amplification and working memory.
#![cfg_attr(not(feature = "std"), no_std)]
#![allow(clippy::missing_const_for_fn)]

extern crate alloc;
use alloc::string::String;

use ndic_core::Result;

/// The registered Zarr v3 array-to-array codec identifier.
pub const CODEC_NAME: &str = "nd_lift";

/// Per-axis transform kind for the `nd_lift` codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftKind {
    /// First-order delta along the axis (`residual[i] = x[i] - x[i-1]`).
    Delta,
    /// Reversible integer Haar lifting.
    Haar,
    /// Reversible integer 5/3-style lifting.
    Lift53,
}

impl LiftKind {
    /// The identifier used in codec configuration JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delta => "delta",
            Self::Haar => "haar",
            Self::Lift53 => "lift53",
        }
    }
}

/// One decorrelation step of the `nd_lift` codec configuration: which axis
/// (by post-transpose index), which transform, how many lifting levels, and
/// the group length the transform is bounded to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisTransform {
    /// Axis name (e.g. `"z"`, `"t"`), for readable configurations.
    pub axis: String,
    /// Axis index into the (post-transpose) chunk shape.
    pub dimension: usize,
    /// Transform kind.
    pub kind: LiftKind,
    /// Decomposition levels (ignored for `Delta`; ≥1 for lifting kinds).
    pub levels: u8,
    /// Group length along the axis (0 = whole chunk extent).
    pub group: u32,
}

/// Forward transform of one chunk in place. Roadmap Phase 2.
///
/// # Errors
/// Returns [`ndic_core::Error::Unsupported`] until Phase 2 lands.
pub fn forward(_chunk: &mut [i32], _shape: &[usize], _steps: &[AxisTransform]) -> Result<()> {
    Err(ndic_core::Error::Unsupported {
        message: "nd_lift forward: implemented in roadmap Phase 2".into(),
    })
}

/// Inverse transform of one chunk in place. Roadmap Phase 2.
///
/// # Errors
/// Returns [`ndic_core::Error::Unsupported`] until Phase 2 lands.
pub fn inverse(_chunk: &mut [i32], _shape: &[usize], _steps: &[AxisTransform]) -> Result<()> {
    Err(ndic_core::Error::Unsupported {
        message: "nd_lift inverse: implemented in roadmap Phase 2".into(),
    })
}
