//! `ndic-lift` — explicit ND decorrelation transforms (`nd_lift`).
//!
//! The cross-axis half of the nd-image-codecs design: an **independently
//! specified** set of integer lifting transforms applied along
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
//!   (roadmap Phase 4).
//!
//! Transforms are applied within bounded **groups** along each axis (e.g.
//! 8–32 samples), capping decode amplification and working memory. Use
//! [`forward`]/[`inverse`] to transform a chunk in its widened coefficient
//! plane (`i32` for input of at most 32 bits, `i64` for 64-bit input).
#![cfg_attr(not(feature = "std"), no_std)]
#![allow(clippy::missing_const_for_fn)]

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ndic_core::{Error, Result};

mod chunk;
mod kernel;
mod sample;

#[cfg(feature = "bench")]
mod bench;

pub use chunk::{forward, inverse};
pub use sample::PlaneSample;

/// The registered Zarr v3 array-to-array codec identifier.
pub const CODEC_NAME: &str = "nd_lift";

/// The `nd_lift` configuration version this build implements.
///
/// The version pins the transform semantics: predictor, update rule,
/// rounding, boundary extension, coefficient ordering, and overflow
/// behaviour. Configurations carrying any other `major.minor` are refused
/// rather than risking a silent mis-decode (under `0.x`, a minor bump *is* a
/// semantics change).
pub const CODEC_VERSION: &str = "0.1";

/// Per-axis transform kind for the `nd_lift` codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct AxisTransform {
    /// Axis name (e.g. `"z"`, `"t"`), for readable configurations.
    #[cfg_attr(feature = "serde", serde(default))]
    pub axis: String,
    /// Axis index into the (post-transpose) chunk shape.
    pub dimension: usize,
    /// Transform kind.
    pub kind: LiftKind,
    /// Decomposition levels (ignored for `Delta`; ≥1 for lifting kinds).
    #[cfg_attr(feature = "serde", serde(default))]
    pub levels: u8,
    /// Group length along the axis (0 = whole chunk extent).
    #[cfg_attr(feature = "serde", serde(default))]
    pub group: u32,
}

/// The `nd_lift` codec `configuration` object.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct NdLiftConfig {
    /// Transform-semantics version; see [`CODEC_VERSION`].
    pub version: String,
    /// The decorrelation steps, in encode order.
    pub transforms: Vec<AxisTransform>,
}

impl NdLiftConfig {
    /// A current-version configuration over `transforms`.
    pub fn new(transforms: Vec<AxisTransform>) -> Self {
        Self {
            version: CODEC_VERSION.into(),
            transforms,
        }
    }

    /// Check everything that does not need a chunk shape: the version gate
    /// and per-transform level requirements.
    ///
    /// # Errors
    /// [`Error::Unsupported`] for an unknown version;
    /// [`Error::InvalidArgument`] for a lifting kind with `levels == 0`.
    pub fn validate_semantics(&self) -> Result<()> {
        check_version(&self.version)?;
        for step in &self.transforms {
            if matches!(step.kind, LiftKind::Haar | LiftKind::Lift53) && step.levels == 0 {
                return Err(Error::InvalidArgument {
                    message: format!(
                        "nd_lift transform on axis {:?}: kind {:?} needs levels >= 1",
                        step.axis,
                        step.kind.as_str()
                    ),
                });
            }
        }
        Ok(())
    }

    /// Check the version and every transform against a chunk dimensionality.
    ///
    /// # Errors
    /// [`Error::Unsupported`] for an unknown version;
    /// [`Error::InvalidArgument`] for an out-of-range `dimension` or a
    /// lifting kind with `levels == 0`.
    pub fn validate(&self, ndim: usize) -> Result<()> {
        self.validate_semantics()?;
        for step in &self.transforms {
            if step.dimension >= ndim {
                return Err(Error::InvalidArgument {
                    message: format!(
                        "nd_lift transform on axis {:?}: dimension {} out of range for a \
                         {ndim}-dimensional chunk",
                        step.axis, step.dimension
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Refuse configuration versions this build does not implement.
fn check_version(version: &str) -> Result<()> {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|p| p.parse::<u32>().ok());
    let minor = parts.next().and_then(|p| p.parse::<u32>().ok());
    if major == Some(0) && minor == Some(1) {
        Ok(())
    } else {
        Err(Error::Unsupported {
            message: format!(
                "nd_lift configuration version {version:?} is not supported by this build \
                 (implements {CODEC_VERSION}); refusing rather than mis-decoding"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn config(version: &str) -> NdLiftConfig {
        NdLiftConfig {
            version: version.to_string(),
            transforms: vec![AxisTransform {
                axis: "z".to_string(),
                dimension: 0,
                kind: LiftKind::Lift53,
                levels: 2,
                group: 0,
            }],
        }
    }

    #[test]
    fn version_gate() {
        assert!(config("0.1").validate(1).is_ok());
        assert!(config("0.1.9").validate(1).is_ok());
        for bad in ["0.2", "1.0", "1.1", "0", "nonsense", ""] {
            assert!(
                matches!(config(bad).validate(1), Err(Error::Unsupported { .. })),
                "version {bad:?} must be refused"
            );
        }
    }

    #[test]
    fn validate_rejects_bad_transforms() {
        let mut cfg = config("0.1");
        cfg.transforms[0].dimension = 3;
        assert!(cfg.validate(2).is_err());
        let mut cfg = config("0.1");
        cfg.transforms[0].levels = 0;
        assert!(cfg.validate(1).is_err());
        cfg.transforms[0].kind = LiftKind::Delta;
        assert!(cfg.validate(1).is_ok(), "delta ignores levels");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn config_serde_round_trip() {
        let json = r#"{
            "version": "0.1",
            "transforms": [
                { "axis": "z", "dimension": 2, "kind": "lift53", "levels": 2, "group": 0 },
                { "axis": "t", "dimension": 1, "kind": "delta", "levels": 0, "group": 8 }
            ]
        }"#;
        let cfg: NdLiftConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.transforms.len(), 2);
        assert_eq!(cfg.transforms[0].kind, LiftKind::Lift53);
        assert_eq!(cfg.transforms[1].kind, LiftKind::Delta);
        assert_eq!(cfg.transforms[1].group, 8);
        let back: NdLiftConfig =
            serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back, cfg);
        // Unknown fields refuse to parse rather than silently dropping
        // semantics.
        assert!(
            serde_json::from_str::<NdLiftConfig>(
                r#"{ "version": "0.1", "transforms": [], "quantize": true }"#
            )
            .is_err()
        );
    }
}
