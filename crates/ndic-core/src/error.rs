//! Error taxonomy shared across the nd-image-codecs crates.

use thiserror::Error;

/// Convenience alias used throughout the workspace.
pub type Result<T> = core::result::Result<T, Error>;

/// The one error type every nd-image-codecs public API returns.
///
/// Variants are intentionally coarse; carry human context in the `message`
/// fields rather than proliferating one variant per call site.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// A caller passed an argument that violates a documented precondition
    /// (out-of-range level count, zero-size code-block, unsupported dtype …).
    #[error("invalid argument: {message}")]
    InvalidArgument {
        /// Human-readable detail.
        message: AllocString,
    },

    /// The codestream (or a marker segment within it) is malformed or truncated.
    #[error("malformed codestream at byte {offset}: {message}")]
    Codestream {
        /// Byte offset into the codestream where parsing failed.
        offset: usize,
        /// Human-readable detail.
        message: AllocString,
    },

    /// A feature is recognized but not yet implemented in this build.
    #[error("unsupported: {message}")]
    Unsupported {
        /// Human-readable detail.
        message: AllocString,
    },

    /// An underlying I/O or storage backend failed (native `std::io`, HTTP
    /// range fetch, Zarr store …). The source is stringified so the core crate
    /// stays `no_std`-compatible.
    #[error("io: {message}")]
    Io {
        /// Human-readable detail.
        message: AllocString,
    },
}

// `alloc::string::String` under a short alias so the doc-comments above read
// cleanly and the crate compiles both with and without `std`.
#[cfg(feature = "std")]
type AllocString = std::string::String;
#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(not(feature = "std"))]
type AllocString = alloc::string::String;
