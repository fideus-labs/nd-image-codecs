//! Whole-chunk forward/inverse transforms: axis iteration, bounded groups,
//! and the encode-time overflow budget.

use alloc::format;
use alloc::vec;

use ndic_core::{Error, Result};

use crate::kernel;
use crate::sample::PlaneSample;
use crate::{AxisTransform, LiftKind};

/// Forward-transform one chunk in place.
///
/// `chunk` is the widened coefficient plane (`i32` for input of at most 32
/// bits, `i64` for 64-bit input) holding the chunk's samples in row-major
/// order for `shape`. Each [`AxisTransform`] in `steps` is applied in listed
/// order along its `dimension`, independently within bounded `group`s.
///
/// Before transforming, the per-axis worst-case bit growth is checked against
/// the plane type (the *overflow budget*): the actual input range is
/// propagated through every step, including the lifting intermediates, and
/// the transform is refused if any value could leave the plane's range.
///
/// # Errors
/// [`Error::InvalidArgument`] when `chunk` does not match `shape`, a step
/// references an out-of-range dimension, a lifting kind has `levels == 0`, or
/// the overflow budget is exceeded.
pub fn forward<T: PlaneSample>(
    chunk: &mut [T],
    shape: &[usize],
    steps: &[AxisTransform],
) -> Result<()> {
    validate(chunk.len(), shape, steps)?;
    check_budget::<T>(chunk, shape, steps)?;
    for step in steps {
        apply_axis(chunk, shape, step, true);
    }
    Ok(())
}

/// Inverse-transform one chunk in place, undoing [`forward`] exactly: the
/// steps run in reverse order, each with its inverse kernel.
///
/// No budget check runs on decode — a chunk produced by [`forward`] cannot
/// overflow on the way back.
///
/// # Errors
/// [`Error::InvalidArgument`] when `chunk` does not match `shape`, a step
/// references an out-of-range dimension, or a lifting kind has `levels == 0`.
pub fn inverse<T: PlaneSample>(
    chunk: &mut [T],
    shape: &[usize],
    steps: &[AxisTransform],
) -> Result<()> {
    validate(chunk.len(), shape, steps)?;
    for step in steps.iter().rev() {
        apply_axis(chunk, shape, step, false);
    }
    Ok(())
}

/// Shared structural validation for [`forward`] and [`inverse`].
fn validate(len: usize, shape: &[usize], steps: &[AxisTransform]) -> Result<()> {
    let expected = shape
        .iter()
        .try_fold(1usize, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| Error::InvalidArgument {
            message: format!("chunk shape {shape:?} overflows usize"),
        })?;
    if len != expected {
        return Err(Error::InvalidArgument {
            message: format!("chunk length {len} != product of shape {shape:?}"),
        });
    }
    for step in steps {
        if step.dimension >= shape.len() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "nd_lift transform on axis {:?}: dimension {} out of range for a \
                     {}-dimensional chunk",
                    step.axis,
                    step.dimension,
                    shape.len()
                ),
            });
        }
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

/// Apply one step along its axis, forward or inverse.
fn apply_axis<T: PlaneSample>(chunk: &mut [T], shape: &[usize], step: &AxisTransform, fwd: bool) {
    let len = shape[step.dimension];
    if len < 2 {
        // Singleton axes are a defensive no-op.
        return;
    }
    let inner: usize = shape[step.dimension + 1..].iter().product();
    let outer: usize = shape[..step.dimension].iter().product();
    let group = effective_group(step.group, len);
    let mut line = vec![T::ZERO; len];
    let mut tmp = vec![T::ZERO; group];
    for o in 0..outer {
        let base = o * len * inner;
        for i in 0..inner {
            for (k, slot) in line.iter_mut().enumerate() {
                *slot = chunk[base + k * inner + i];
            }
            for seg in line.chunks_mut(group) {
                let tmp = &mut tmp[..seg.len()];
                match (step.kind, fwd) {
                    (LiftKind::Delta, true) => kernel::delta_forward(seg),
                    (LiftKind::Delta, false) => kernel::delta_inverse(seg),
                    (LiftKind::Haar, true) => kernel::haar_forward(seg, tmp, step.levels),
                    (LiftKind::Haar, false) => kernel::haar_inverse(seg, tmp, step.levels),
                    (LiftKind::Lift53, true) => kernel::lift53_forward(seg, tmp, step.levels),
                    (LiftKind::Lift53, false) => kernel::lift53_inverse(seg, tmp, step.levels),
                }
            }
            for (k, &v) in line.iter().enumerate() {
                chunk[base + k * inner + i] = v;
            }
        }
    }
}

/// The bounded segment length a step couples along its axis.
fn effective_group(group: u32, len: usize) -> usize {
    if group == 0 {
        len
    } else {
        (group as usize).min(len)
    }
}

/// An inclusive value interval, tracked in `i128` so the budget arithmetic
/// itself cannot overflow (plane values are at most 64-bit).
#[derive(Clone, Copy)]
struct Interval {
    lo: i128,
    hi: i128,
}

impl Interval {
    fn union(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    fn fits<T: PlaneSample>(self) -> bool {
        self.lo >= T::MIN_I128 && self.hi <= T::MAX_I128
    }
}

const fn floor_div(a: i128, b: i128) -> i128 {
    a.div_euclid(b)
}

/// Refuse encodes whose worst-case coefficient (or lifting intermediate)
/// could leave the plane type's range: the per-axis bit-growth assertion.
fn check_budget<T: PlaneSample>(
    chunk: &[T],
    shape: &[usize],
    steps: &[AxisTransform],
) -> Result<()> {
    let Some(first) = chunk.first() else {
        return Ok(());
    };
    let mut all = Interval {
        lo: first.to_i128(),
        hi: first.to_i128(),
    };
    for &v in chunk {
        let v = v.to_i128();
        all.lo = all.lo.min(v);
        all.hi = all.hi.max(v);
    }
    for step in steps {
        let len = shape.get(step.dimension).copied().unwrap_or(0);
        if len < 2 {
            continue;
        }
        let grown = step_growth::<T>(all, step, effective_group(step.group, len));
        let Some(grown) = grown else {
            return Err(Error::InvalidArgument {
                message: format!(
                    "nd_lift overflow budget exceeded on axis {:?} ({} levels of {:?} over \
                     input range [{}, {}]): coefficients would leave the {}-bit plane; \
                     reduce levels or shorten the group",
                    step.axis,
                    step.levels.max(1),
                    step.kind.as_str(),
                    all.lo,
                    all.hi,
                    if T::MAX_I128 == i128::from(i32::MAX) {
                        32
                    } else {
                        64
                    },
                ),
            });
        };
        all = grown;
    }
    Ok(())
}

/// Propagate `input` through one step's worst case; `None` when any value or
/// intermediate could leave the plane range.
fn step_growth<T: PlaneSample>(
    input: Interval,
    step: &AxisTransform,
    group: usize,
) -> Option<Interval> {
    let levels = match step.kind {
        LiftKind::Delta => 1,
        LiftKind::Haar | LiftKind::Lift53 => dyadic_depth(group, step.levels),
    };
    let mut all = input;
    let mut band = input;
    for _ in 0..levels {
        let (lo, hi) = (band.lo, band.hi);
        let (next_band, level_all) = match step.kind {
            // r = x − prev ∪ r[0] = x[0]; no wider intermediates.
            LiftKind::Delta => {
                let r = Interval {
                    lo: lo - hi,
                    hi: hi - lo,
                };
                (input, r.union(input))
            }
            // d = x₁ − x₀; s = x₀ + ⌊d/2⌋; no wider intermediates.
            LiftKind::Haar => {
                let d = Interval {
                    lo: lo - hi,
                    hi: hi - lo,
                };
                let s = Interval {
                    lo: lo + floor_div(d.lo, 2),
                    hi: hi + floor_div(d.hi, 2),
                };
                (s, s.union(d))
            }
            // Intermediates x₀+x₂+1 and d₋+d₊+2 must fit the plane too.
            LiftKind::Lift53 => {
                let predict_sum = Interval {
                    lo: 2 * lo + 1,
                    hi: 2 * hi + 1,
                };
                let d = Interval {
                    lo: lo - hi,
                    hi: hi - lo,
                };
                let update_sum = Interval {
                    lo: 2 * d.lo + 2,
                    hi: 2 * d.hi + 2,
                };
                if !predict_sum.fits::<T>() || !update_sum.fits::<T>() {
                    return None;
                }
                let s = Interval {
                    lo: lo + floor_div(update_sum.lo, 4),
                    hi: hi + floor_div(update_sum.hi, 4),
                };
                (s, s.union(d))
            }
        };
        all = all.union(level_all);
        if !all.fits::<T>() {
            return None;
        }
        band = next_band;
    }
    Some(all)
}

/// How many dyadic levels a decomposition actually applies to `len` samples.
fn dyadic_depth(len: usize, levels: u8) -> u32 {
    let mut m = len;
    let mut applied = 0;
    for _ in 0..levels {
        if m < 2 {
            break;
        }
        applied += 1;
        m = m.div_ceil(2);
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec::Vec;

    fn step(dimension: usize, kind: LiftKind, levels: u8, group: u32) -> AxisTransform {
        AxisTransform {
            axis: "z".to_string(),
            dimension,
            kind,
            levels,
            group,
        }
    }

    #[test]
    fn delta_along_z_of_volume() {
        // 2×2×3 volume, delta along axis 0 (stride = 6).
        let mut chunk: Vec<i32> = (0..12).collect();
        let shape = [2, 2, 3];
        forward(&mut chunk, &shape, &[step(0, LiftKind::Delta, 0, 0)]).unwrap();
        // Second z-slice becomes the constant difference 6.
        assert_eq!(chunk, vec![0, 1, 2, 3, 4, 5, 6, 6, 6, 6, 6, 6]);
        inverse(&mut chunk, &shape, &[step(0, LiftKind::Delta, 0, 0)]).unwrap();
        assert_eq!(chunk, (0..12).collect::<Vec<i32>>());
    }

    #[test]
    fn groups_bound_coupling() {
        // Delta over groups of 2: each pair restarts, so residual[2] = x[2].
        let mut chunk: Vec<i32> = vec![10, 11, 20, 23];
        forward(&mut chunk, &[4], &[step(0, LiftKind::Delta, 0, 2)]).unwrap();
        assert_eq!(chunk, vec![10, 1, 20, 3]);
        inverse(&mut chunk, &[4], &[step(0, LiftKind::Delta, 0, 2)]).unwrap();
        assert_eq!(chunk, vec![10, 11, 20, 23]);
    }

    #[test]
    fn listed_order_applies_and_reverses() {
        let shape = [4, 4];
        let steps = [
            step(0, LiftKind::Lift53, 2, 0),
            step(1, LiftKind::Haar, 1, 0),
        ];
        let original: Vec<i32> = (0..16).map(|i| (i * 7) % 23 - 11).collect();
        let mut chunk = original.clone();
        forward(&mut chunk, &shape, &steps).unwrap();
        assert_ne!(chunk, original);
        inverse(&mut chunk, &shape, &steps).unwrap();
        assert_eq!(chunk, original);
    }

    #[test]
    fn singleton_axis_is_a_noop() {
        let mut chunk: Vec<i32> = vec![7, 8, 9];
        forward(&mut chunk, &[1, 3], &[step(0, LiftKind::Lift53, 2, 0)]).unwrap();
        assert_eq!(chunk, vec![7, 8, 9]);
    }

    #[test]
    fn shape_and_dimension_mismatches_error() {
        let mut chunk = vec![0i32; 5];
        assert!(forward(&mut chunk, &[2, 3], &[]).is_err());
        let mut chunk = vec![0i32; 6];
        assert!(forward(&mut chunk, &[2, 3], &[step(2, LiftKind::Delta, 0, 0)]).is_err());
        assert!(forward(&mut chunk, &[2, 3], &[step(0, LiftKind::Haar, 0, 0)]).is_err());
    }

    #[test]
    fn budget_refuses_full_range_i32_planes() {
        let mut chunk = vec![i32::MIN, i32::MAX];
        let err = forward(&mut chunk, &[2], &[step(0, LiftKind::Delta, 0, 0)]).unwrap_err();
        assert!(err.to_string().contains("overflow budget"), "{err}");
        // The same values are comfortable in an i64 plane.
        let mut chunk = vec![i64::from(i32::MIN), i64::from(i32::MAX)];
        forward(&mut chunk, &[2], &[step(0, LiftKind::Delta, 0, 0)]).unwrap();
    }

    #[test]
    fn budget_allows_16_bit_input_in_i32_planes() {
        let mut chunk = vec![i32::from(u16::MAX), 0, i32::from(u16::MAX), 0];
        forward(&mut chunk, &[4], &[step(0, LiftKind::Lift53, 3, 0)]).unwrap();
    }
}
