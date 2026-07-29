//! In-memory views over coefficient planes and volumes.
//!
//! These are deliberately borrow-based (no ownership, no allocation) so the
//! block coder and the transform stages can operate on caller-owned buffers.
//! The canonical in-memory layout is row-major `[z, y, x]` with `x` fastest —
//! the same slowest-to-fastest axis order used by `NumPy`, Zarr, and OME-NGFF
//! (each format reader/writer owns any axis reversal at its own boundary).

/// A single 2D plane of wavelet coefficients (`i32`), row-major with `x`
/// fastest. This is the unit the HT block coder consumes: after the in-plane
/// 2D DWT (and, when enabled, the `nd_lift` cross-axis transform) each subband plane is
/// partitioned into code-blocks and coded independently.
#[derive(Debug, Clone, Copy)]
pub struct CoeffPlane<'a> {
    /// Coefficient samples, `height * width` in row-major order.
    pub samples: &'a [i32],
    /// Plane width (x extent).
    pub width: usize,
    /// Plane height (y extent).
    pub height: usize,
}

impl<'a> CoeffPlane<'a> {
    /// Construct a view, checking that `samples.len() == width * height`.
    ///
    /// # Errors
    /// Returns [`crate::Error::InvalidArgument`] on a length mismatch.
    pub fn new(samples: &'a [i32], width: usize, height: usize) -> crate::Result<Self> {
        if samples.len() == width.saturating_mul(height) {
            Ok(Self {
                samples,
                width,
                height,
            })
        } else {
            Err(crate::Error::InvalidArgument {
                message: format_len_mismatch(samples.len(), width, height),
            })
        }
    }
}

/// A borrowed volume: `depth` planes of `width × height`, row-major `[z,y,x]`.
///
/// The z-axis is the slice direction the `nd_lift` transform decorrelates.
#[derive(Debug, Clone, Copy)]
pub struct VolumeView<'a> {
    /// Flattened samples, `depth * height * width` in `[z, y, x]` order.
    pub samples: &'a [i32],
    /// Number of slices (z extent).
    pub depth: usize,
    /// Plane width (x extent).
    pub width: usize,
    /// Plane height (y extent).
    pub height: usize,
}

impl VolumeView<'_> {
    /// Number of samples in one z-slice.
    #[must_use]
    pub const fn plane_len(&self) -> usize {
        self.width * self.height
    }
}

#[cfg(feature = "std")]
fn format_len_mismatch(got: usize, w: usize, h: usize) -> std::string::String {
    format!("plane length {got} != width {w} * height {h}")
}
#[cfg(not(feature = "std"))]
fn format_len_mismatch(_got: usize, _w: usize, _h: usize) -> &'static str {
    "plane length != width * height"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_rejects_wrong_len() {
        let buf = [0i32; 6];
        assert!(CoeffPlane::new(&buf, 2, 3).is_ok());
        assert!(CoeffPlane::new(&buf, 2, 2).is_err());
    }
}
