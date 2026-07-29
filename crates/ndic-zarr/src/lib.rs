//! `ndic-zarr` — Zarr v3 codecs and the codec-series builder.
//!
//! nd-image-codecs contributes two registered Zarr v3 codecs and composes a
//! third family from existing codecs:
//!
//! | Codec | Kind | Crate |
//! | --- | --- | --- |
//! | [`ndic_lift::CODEC_NAME`] (`nd_lift`) | array → array | `ndic-lift` |
//! | [`HTJ2K_CODEC_NAME`] (`htj2k`) | array → bytes | `ndic-htj2k` + `ndic-codestream` |
//! | [`ndic_zfp::CODEC_NAME`] (`nd_zfp`) | array → bytes | `ndic-zfp` |
//!
//! The [`series`] module builds complete codec **pipelines** ("series") from
//! axis metadata — `transpose` → decorrelation → plane/block codec — for the
//! three families **nd-delta**, **nd-lift-ht**, and **nd-zfp**. See
//! `docs/architecture/codec-series.md`.
//!
//! Codecs register into the `zarrs` plugin registry via `inventory` when the
//! `zarrs` feature is enabled (roadmap Phases 2–5).

pub mod series;

/// The registered Zarr v3 array-to-bytes codec identifier for the HTJ2K
/// coefficient-plane codec (independent Part 1 / Part 15 codestreams per
/// trailing 2D plane, plus a coefficient-plane byte index for range access).
pub const HTJ2K_CODEC_NAME: &str = "htj2k";

pub use ndic_lift::CODEC_NAME as ND_LIFT_CODEC_NAME;
pub use ndic_zfp::CODEC_NAME as ND_ZFP_CODEC_NAME;
