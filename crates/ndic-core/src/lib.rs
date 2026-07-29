//! `ndic-core` — shared, dependency-light foundations for the nd-image-codecs
//! codec stack.
//!
//! This crate holds the types every other nd-image-codecs crate depends on and that
//! carry **no** codec logic themselves: the error taxonomy, the sample dtype
//! enum, the in-memory tile/plane/volume views, and the parameter structs that
//! describe a codestream (resolution levels, code-block size, progression
//! order, quality layers, precincts). Keeping these free of the block coder,
//! the wavelet transform, and the codestream reader/writer lets the workspace
//! form an acyclic dependency graph:
//!
//! ```text
//! ndic-core ← {ndic-lift, ndic-zfp, ndic-htj2k ← ndic-codestream}
//!      ▲                              │
//!      └──────── ndic-{zarr, cli} ◄───┘
//! ```
//!
//! Everything here is `no_std`-friendly behind the default `std` feature so the
//! browser WASM binding can drop `std` where the toolchain allows.
#![cfg_attr(not(feature = "std"), no_std)]

mod dtype;
mod error;
mod params;
mod plane;

pub use dtype::SampleType;
pub use error::{Error, Result};
pub use params::{CodeBlockStyle, EncodeParams, ProgressionOrder, WaveletKind};
pub use plane::{CoeffPlane, VolumeView};
