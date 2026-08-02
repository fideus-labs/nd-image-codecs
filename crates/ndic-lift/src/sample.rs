//! The widened integer plane types the lifting kernels operate on.

use core::fmt::Debug;
use core::ops::{Add, AddAssign, Shr, Sub, SubAssign};

/// An integer coefficient-plane sample type.
///
/// `nd_lift` transforms input samples in a **widened** signed plane so the
/// lifting arithmetic is exact: `i32` for inputs of at most 32 bits and `i64`
/// for 64-bit inputs (see `docs/architecture/nd-transform.md`). The
/// [`forward`](crate::forward) budget check guarantees every kernel
/// expression — including the `x₀ + x₂ + 1` and `d₋ + d₊ + 2` intermediates —
/// stays inside the plane's range, so the kernels use plain (non-widening)
/// arithmetic.
///
/// `>>` must be an *arithmetic* shift (Rust's `>>` on signed integers), which
/// the kernels rely on for floor division by powers of two.
pub trait PlaneSample:
    Copy
    + Ord
    + Debug
    + 'static
    + Add<Output = Self>
    + Sub<Output = Self>
    + AddAssign
    + SubAssign
    + Shr<u32, Output = Self>
{
    /// The additive identity.
    const ZERO: Self;
    /// One (the 5/3 predict rounding offset).
    const ONE: Self;
    /// Two (the 5/3 update rounding offset).
    const TWO: Self;
    /// The plane's minimum value, widened for overflow-budget arithmetic.
    const MIN_I128: i128;
    /// The plane's maximum value, widened for overflow-budget arithmetic.
    const MAX_I128: i128;

    /// The sample value, widened for overflow-budget arithmetic.
    fn to_i128(self) -> i128;
}

macro_rules! impl_plane_sample {
    ($ty:ty) => {
        impl PlaneSample for $ty {
            const ZERO: Self = 0;
            const ONE: Self = 1;
            const TWO: Self = 2;
            const MIN_I128: i128 = <$ty>::MIN as i128;
            const MAX_I128: i128 = <$ty>::MAX as i128;

            #[inline]
            fn to_i128(self) -> i128 {
                i128::from(self)
            }
        }
    };
}

impl_plane_sample!(i32);
impl_plane_sample!(i64);
