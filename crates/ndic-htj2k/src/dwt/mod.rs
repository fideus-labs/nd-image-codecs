//! In-plane 2D discrete wavelet transform (T.800 Annex F).
//!
//! Two kernels, applied to each 2D plane before HT block coding:
//!
//! - **Le Gall 5/3** ([`forward_53`], [`inverse_53`]): integer lifting,
//!   bit-exact reversible;
//! - **CDF 9/7** ([`forward_97`], [`inverse_97`]): real-valued lifting for
//!   the lossy path. The `K` scaling factor is *not* applied here — like
//!   `OpenJPH`, it is absorbed into the per-subband quantization step.
//!
//! Geometry follows the Part-1 canvas rules for images anchored at the
//! origin: every resolution starts at coordinate 0, so even (low-pass)
//! samples are at indices 0, 2, 4 … . Each decomposition level runs the
//! **vertical pass first, then the horizontal pass** — the same order as
//! `OpenJPH`'s line pipeline, which matters for bit-exactness because
//! integer lifting steps do not commute across axes.
//!
//! Layout is the standard in-place Mallat arrangement: level `l` transforms
//! the current LL region (top-left `w_l x h_l`), leaving `[LL | HL]` over
//! `[LH | HH]`.
//!
//! # Float evaluation order in the 9/7 path
//!
//! [`lift_97`] uses IEEE-strict `+` and `*`. Rust 1.98's `algebraic_add` /
//! `algebraic_mul` — which license the backend to reassociate and contract —
//! were tried here and **reverted after measurement**. Recorded so the
//! experiment is not repeated:
//!
//! - The lifting loops are **elementwise, with no carried reduction**, so
//!   there is no accumulator chain to reassociate. The only transformation the
//!   permission enables is contracting `x + coeff * y` into an FMA.
//! - That contraction needs FMA in the target feature set, and this workspace
//!   builds for the **baseline `x86-64`** target, which has none. On the
//!   shipped profile the algebraic and strict forms compiled to *identical*
//!   instructions and produced *bit-identical* output over 1 M samples.
//! - Under `-C target-cpu=native` the contraction does happen (6 scalar float
//!   ops become 4) and shifts results by ~5.5e-7 of peak coefficient
//!   magnitude — comfortably inside the irreversible quantization step, and in
//!   fact slightly *more* accurate, since an FMA rounds once instead of twice.
//!   It still produced **no measurable speedup**: the loop is bound by the
//!   symmetric-extension index clamps and bounds checks, not by float
//!   throughput.
//! - Neither form vectorizes. The `.min()` / `.saturating_sub()` boundary
//!   clamps on the load indices are what prevent it, and no amount of
//!   arithmetic permission addresses that. Peeling the first and last
//!   iterations so the interior is unconditionally stride-1 is the change that
//!   would actually vectorize this loop.
//!
//! The **reversible 5/3 kernel above is integer lifting** and is out of scope
//! for any of this: its bit-exactness is a correctness requirement, not a
//! tolerance.

pub mod simd;

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use ndic_core::{Error, Result};

/// Returns the LL dimensions after `level` decompositions (ceil division).
#[must_use]
pub const fn level_dims(width: usize, height: usize, level: u8) -> (usize, usize) {
    let mut w = width;
    let mut h = height;
    let mut i = 0;
    while i < level {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        i += 1;
    }
    (w, h)
}

/// One reversible 5/3 analysis of a 1D signal (origin at even parity):
/// evens to `low`, odds to `high`, in place over `buf[..n]` split buffers.
///
/// T.800 F.4.8.2.1 with symmetric extension:
/// `d[i] = x[2i+1] - ((x[2i] + x[2i+2]) >> 1)`,
/// `s[i] = x[2i] + ((d[i-1] + d[i] + 2) >> 2)`.
pub(crate) fn ana_53(x: &[i32], low: &mut [i32], high: &mut [i32]) {
    let n = x.len();
    if n == 1 {
        // Single even sample: passes through unchanged (T.800 eq. F-9).
        low[0] = x[0];
        return;
    }
    let nl = n.div_ceil(2);
    let nh = n / 2;
    // Predict: odd samples become the high band.
    for i in 0..nh {
        let right = if 2 * i + 2 < n {
            x[2 * i + 2]
        } else {
            x[n - 2]
        };
        high[i] = x[2 * i + 1] - ((x[2 * i] + right) >> 1);
    }
    // Update: even samples become the low band.
    for i in 0..nl {
        let dl = high[i.saturating_sub(1).min(nh - 1)];
        let dr = high[i.min(nh - 1)];
        low[i] = x[2 * i] + ((dl + dr + 2) >> 2);
    }
}

/// Inverse of [`ana_53`].
pub(crate) fn syn_53(low: &[i32], high: &[i32], x: &mut [i32]) {
    let n = x.len();
    if n == 1 {
        x[0] = low[0];
        return;
    }
    let nl = n.div_ceil(2);
    let nh = n / 2;
    // Undo update.
    for i in 0..nl {
        let dl = high[i.saturating_sub(1).min(nh - 1)];
        let dr = high[i.min(nh - 1)];
        x[2 * i] = low[i] - ((dl + dr + 2) >> 2);
    }
    // Undo predict.
    for i in 0..nh {
        let right = if 2 * i + 2 < n {
            x[2 * i + 2]
        } else {
            x[n - 2]
        };
        x[2 * i + 1] = high[i] + ((x[2 * i] + right) >> 1);
    }
}

/// The four 9/7 lifting multipliers (T.800 Table F.4, analysis order).
const ALPHA: f32 = -1.586_134_3;
const BETA: f32 = -0.052_980_118;
const GAMMA: f32 = 0.882_911_1;
const DELTA: f32 = 0.443_506_85;

/// One irreversible 9/7 lifting step over de-interleaved buffers.
///
/// The multiply-accumulate stays on IEEE-strict `+` / `*`. Rust 1.98's
/// `f32::algebraic_mul` / `algebraic_add` were measured here and reverted —
/// see the module header for the numbers and the reason.
fn lift_97(low: &mut [f32], high: &mut [f32], n: usize, coeff: f32, high_from_low: bool) {
    let nl = n.div_ceil(2);
    let nh = n / 2;
    if high_from_low {
        // high[i] += coeff * (low[i] + low[i+1]) with symmetric extension.
        for i in 0..nh {
            let l = low[i];
            let r = low[(i + 1).min(nl - 1)];
            high[i] += coeff * (l + r);
        }
    } else {
        // low[i] += coeff * (high[i-1] + high[i]) with symmetric extension.
        for i in 0..nl {
            let l = high[i.saturating_sub(1).min(nh - 1)];
            let r = high[i.min(nh - 1)];
            low[i] += coeff * (l + r);
        }
    }
}

/// Applies `levels` 5/3 decompositions in place over `plane`.
///
/// # Errors
/// [`Error::InvalidArgument`] on geometry mismatch.
pub fn forward_53(plane: &mut [i32], width: usize, height: usize, levels: u8) -> Result<()> {
    check_geometry(plane.len(), width, height)?;
    let mut scratch = Scratch::new(width, height);
    for l in 0..levels {
        let (w, h) = level_dims(width, height, l);
        if w == 1 && h == 1 {
            break;
        }
        // Vertical pass first (matches the reference line pipeline).
        if h > 1 {
            for x in 0..w {
                scratch.gather_col(plane, width, x, h);
                let (sig, low, high) = scratch.split(h);
                ana_53(sig, low, high);
                scratch.scatter_col_split(plane, width, x, h);
            }
        } else {
            // A single row at an odd vertical position would double; at the
            // origin the row is even and passes through (T.800 eq. F-9).
        }
        // Horizontal pass.
        if w > 1 {
            for y in 0..h {
                scratch.gather_row(plane, width, y, w);
                let (sig, low, high) = scratch.split(w);
                ana_53(sig, low, high);
                scratch.scatter_row_split(plane, width, y, w);
            }
        }
    }
    Ok(())
}

/// Inverse of [`forward_53`].
///
/// # Errors
/// [`Error::InvalidArgument`] on geometry mismatch.
pub fn inverse_53(plane: &mut [i32], width: usize, height: usize, levels: u8) -> Result<()> {
    check_geometry(plane.len(), width, height)?;
    let mut scratch = Scratch::new(width, height);
    for l in (0..levels).rev() {
        let (w, h) = level_dims(width, height, l);
        if w == 1 && h == 1 {
            continue;
        }
        // Horizontal synthesis first (inverse order of the analysis).
        if w > 1 {
            for y in 0..h {
                scratch.gather_row_split(plane, width, y, w);
                let (sig, low, high) = scratch.split(w);
                syn_53(low, high, sig);
                scratch.scatter_row(plane, width, y, w);
            }
        }
        // Vertical synthesis.
        if h > 1 {
            for x in 0..w {
                scratch.gather_col_split(plane, width, x, h);
                let (sig, low, high) = scratch.split(h);
                syn_53(low, high, sig);
                scratch.scatter_col(plane, width, x, h);
            }
        }
    }
    Ok(())
}

/// Applies `levels` 9/7 decompositions in place over an `f32` plane.
///
/// # Errors
/// [`Error::InvalidArgument`] on geometry mismatch.
pub fn forward_97(plane: &mut [f32], width: usize, height: usize, levels: u8) -> Result<()> {
    check_geometry(plane.len(), width, height)?;
    let mut col = vec![0f32; width.max(height)];
    let mut low = vec![0f32; width.max(height).div_ceil(2)];
    let mut high = vec![0f32; width.max(height) / 2 + 1];
    for l in 0..levels {
        let (w, h) = level_dims(width, height, l);
        if w == 1 && h == 1 {
            break;
        }
        if h > 1 {
            for x in 0..w {
                for y in 0..h {
                    col[y] = plane[y * width + x];
                }
                ana_97_1d(&col[..h], &mut low, &mut high);
                let nl = h.div_ceil(2);
                for y in 0..nl {
                    plane[y * width + x] = low[y];
                }
                for y in 0..h / 2 {
                    plane[(nl + y) * width + x] = high[y];
                }
            }
        }
        if w > 1 {
            for y in 0..h {
                col[..w].copy_from_slice(&plane[y * width..y * width + w]);
                ana_97_1d(&col[..w], &mut low, &mut high);
                let nl = w.div_ceil(2);
                plane[y * width..y * width + nl].copy_from_slice(&low[..nl]);
                plane[y * width + nl..y * width + w].copy_from_slice(&high[..w / 2]);
            }
        }
    }
    Ok(())
}

/// Inverse of [`forward_97`].
///
/// # Errors
/// [`Error::InvalidArgument`] on geometry mismatch.
pub fn inverse_97(plane: &mut [f32], width: usize, height: usize, levels: u8) -> Result<()> {
    check_geometry(plane.len(), width, height)?;
    let mut sig = vec![0f32; width.max(height)];
    let mut low = vec![0f32; width.max(height).div_ceil(2)];
    let mut high = vec![0f32; width.max(height) / 2 + 1];
    for l in (0..levels).rev() {
        let (w, h) = level_dims(width, height, l);
        if w == 1 && h == 1 {
            continue;
        }
        if w > 1 {
            for y in 0..h {
                let nl = w.div_ceil(2);
                low[..nl].copy_from_slice(&plane[y * width..y * width + nl]);
                high[..w / 2].copy_from_slice(&plane[y * width + nl..y * width + w]);
                syn_97_1d(&mut sig[..w], &mut low, &mut high);
                plane[y * width..y * width + w].copy_from_slice(&sig[..w]);
            }
        }
        if h > 1 {
            for x in 0..w {
                let nl = h.div_ceil(2);
                for y in 0..nl {
                    low[y] = plane[y * width + x];
                }
                for y in 0..h / 2 {
                    high[y] = plane[(nl + y) * width + x];
                }
                syn_97_1d(&mut sig[..h], &mut low, &mut high);
                for y in 0..h {
                    plane[y * width + x] = sig[y];
                }
            }
        }
    }
    Ok(())
}

/// 1D 9/7 analysis: de-interleave then four lifting steps.
fn ana_97_1d(x: &[f32], low: &mut [f32], high: &mut [f32]) {
    let n = x.len();
    if n == 1 {
        low[0] = x[0];
        return;
    }
    let nl = n.div_ceil(2);
    let nh = n / 2;
    for i in 0..nl {
        low[i] = x[2 * i];
    }
    for i in 0..nh {
        high[i] = x[2 * i + 1];
    }
    lift_97(low, high, n, ALPHA, true);
    lift_97(low, high, n, BETA, false);
    lift_97(low, high, n, GAMMA, true);
    lift_97(low, high, n, DELTA, false);
}

/// 1D 9/7 synthesis: undo the four lifting steps then interleave.
fn syn_97_1d(x: &mut [f32], low: &mut [f32], high: &mut [f32]) {
    let n = x.len();
    if n == 1 {
        x[0] = low[0];
        return;
    }
    let nl = n.div_ceil(2);
    let nh = n / 2;
    lift_97(low, high, n, -DELTA, false);
    lift_97(low, high, n, -GAMMA, true);
    lift_97(low, high, n, -BETA, false);
    lift_97(low, high, n, -ALPHA, true);
    for i in 0..nl {
        x[2 * i] = low[i];
    }
    for i in 0..nh {
        x[2 * i + 1] = high[i];
    }
}

pub(crate) fn check_geometry(len: usize, width: usize, height: usize) -> Result<()> {
    if width == 0 || height == 0 || len < width * height {
        return Err(Error::InvalidArgument {
            message: "plane buffer smaller than width * height".into(),
        });
    }
    Ok(())
}

/// Reusable gather/scatter buffers for the line-oriented integer passes.
pub(crate) struct Scratch {
    /// Interleaved signal.
    sig: Vec<i32>,
    /// De-interleaved low band.
    low: Vec<i32>,
    /// De-interleaved high band.
    high: Vec<i32>,
}

impl Scratch {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        let m = width.max(height);
        Self {
            sig: vec![0; m],
            low: vec![0; m.div_ceil(2)],
            high: vec![0; m / 2 + 1],
        }
    }

    pub(crate) fn split(&mut self, n: usize) -> (&mut [i32], &mut [i32], &mut [i32]) {
        (
            &mut self.sig[..n],
            &mut self.low[..n.div_ceil(2)],
            &mut self.high[..n / 2],
        )
    }

    fn gather_col(&mut self, plane: &[i32], width: usize, x: usize, h: usize) {
        for y in 0..h {
            self.sig[y] = plane[y * width + x];
        }
    }

    fn scatter_col_split(&self, plane: &mut [i32], width: usize, x: usize, h: usize) {
        let nl = h.div_ceil(2);
        for y in 0..nl {
            plane[y * width + x] = self.low[y];
        }
        for y in 0..h / 2 {
            plane[(nl + y) * width + x] = self.high[y];
        }
    }

    fn gather_col_split(&mut self, plane: &[i32], width: usize, x: usize, h: usize) {
        let nl = h.div_ceil(2);
        for y in 0..nl {
            self.low[y] = plane[y * width + x];
        }
        for y in 0..h / 2 {
            self.high[y] = plane[(nl + y) * width + x];
        }
    }

    fn scatter_col(&self, plane: &mut [i32], width: usize, x: usize, h: usize) {
        for y in 0..h {
            plane[y * width + x] = self.sig[y];
        }
    }

    pub(crate) fn gather_row(&mut self, plane: &[i32], width: usize, y: usize, w: usize) {
        self.sig[..w].copy_from_slice(&plane[y * width..y * width + w]);
    }

    pub(crate) fn scatter_row_split(&self, plane: &mut [i32], width: usize, y: usize, w: usize) {
        let nl = w.div_ceil(2);
        plane[y * width..y * width + nl].copy_from_slice(&self.low[..nl]);
        plane[y * width + nl..y * width + w].copy_from_slice(&self.high[..w / 2]);
    }

    pub(crate) fn gather_row_split(&mut self, plane: &[i32], width: usize, y: usize, w: usize) {
        let nl = w.div_ceil(2);
        self.low[..nl].copy_from_slice(&plane[y * width..y * width + nl]);
        self.high[..w / 2].copy_from_slice(&plane[y * width + nl..y * width + w]);
    }

    pub(crate) fn scatter_row(&self, plane: &mut [i32], width: usize, y: usize, w: usize) {
        plane[y * width..y * width + w].copy_from_slice(&self.sig[..w]);
    }
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)] // small test values fit f32 exactly
mod tests {
    use super::*;

    fn roundtrip_53(plane: &[i32], w: usize, h: usize, levels: u8) {
        let mut p = plane.to_vec();
        forward_53(&mut p, w, h, levels).unwrap();
        inverse_53(&mut p, w, h, levels).unwrap();
        assert_eq!(p, plane, "{w}x{h} levels={levels}");
    }

    #[test]
    fn dc_plane_transforms_to_dc_ll() {
        let mut p = vec![100i32; 16 * 16];
        forward_53(&mut p, 16, 16, 2).unwrap();
        // LL (4x4 top-left) holds the DC value; all detail bands are zero.
        for y in 0..16 {
            for x in 0..16 {
                let want = if x < 4 && y < 4 { 100 } else { 0 };
                assert_eq!(p[y * 16 + x], want, "({x}, {y})");
            }
        }
    }

    #[test]
    fn impulse_at_origin_stays_in_ll() {
        let mut p = vec![0i32; 8 * 8];
        p[0] = 1;
        forward_53(&mut p, 8, 8, 1).unwrap();
        assert_eq!(p[0], 1);
        assert_eq!(p.iter().filter(|&&v| v != 0).count(), 1);
    }

    #[test]
    fn roundtrips_fixed_planes() {
        let ramp: Vec<i32> = (0..64 * 48).map(|i| i % 251 - 125).collect();
        for levels in 0..=5 {
            roundtrip_53(&ramp, 64, 48, levels);
        }
        // Odd sizes, degenerate rows/columns.
        let odd: Vec<i32> = (0..33 * 17).map(|i| (i * 7 % 509) - 254).collect();
        roundtrip_53(&odd, 33, 17, 5);
        roundtrip_53(&odd[..31], 31, 1, 3);
        roundtrip_53(&odd[..31], 1, 31, 3);
        roundtrip_53(&odd[..1], 1, 1, 5);
        roundtrip_53(&odd[..2], 2, 1, 1);
        roundtrip_53(&odd[..3], 3, 1, 2);
    }

    #[test]
    fn roundtrips_extreme_values() {
        // 16-bit extremes with 5 levels stay within i32 (precision budget).
        let extremes: Vec<i32> = (0..64 * 64)
            .map(|i| if i % 2 == 0 { 32767 } else { -32768 })
            .collect();
        roundtrip_53(&extremes, 64, 64, 5);
    }

    #[test]
    fn dwt97_roundtrips_within_tolerance() {
        let plane: Vec<f32> = (0..40 * 24)
            .map(|i| ((i * 13) % 257) as f32 - 128.0)
            .collect();
        let mut p = plane.clone();
        forward_97(&mut p, 40, 24, 3).unwrap();
        inverse_97(&mut p, 40, 24, 3).unwrap();
        for (i, (&want, &got)) in plane.iter().zip(p.iter()).enumerate() {
            assert!((want - got).abs() < 1e-2, "sample {i}: {want} vs {got}");
        }
    }

    #[test]
    fn level_dims_ceil() {
        assert_eq!(level_dims(65, 33, 1), (33, 17));
        assert_eq!(level_dims(65, 33, 5), (3, 2));
        assert_eq!(level_dims(1, 1, 5), (1, 1));
    }

    #[test]
    fn rejects_bad_geometry() {
        let mut p = vec![0i32; 8];
        assert!(forward_53(&mut p, 3, 3, 1).is_err());
        assert!(forward_53(&mut p, 0, 8, 1).is_err());
    }
}
