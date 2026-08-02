"""NumPy reference implementation of the ``nd_lift`` ``0.1`` transform.

A faithful port of the Rust ``ndic-lift`` crate — same predictor, update
rule, rounding, whole-sample symmetric boundary extension, subband
coefficient ordering, grouping, widening, and overflow budget — asserted
bit-identical against the committed conformance vectors
(``fixtures/nd-lift/vectors.json``) that the Rust test suite also enforces.

Transforms operate in a **widened** signed plane: ``int32`` for input dtypes
of at most 32 bits, ``int64`` for 64-bit input. NumPy's ``//`` on signed
integers is floor division, matching the Rust arithmetic shifts. The encode
path propagates the input's value interval through every step (including the
lifting intermediates) and refuses transforms that could leave the plane —
the same per-axis bit-growth assertion the Rust codec applies.
"""

from __future__ import annotations

import numbers
from collections.abc import Mapping
from typing import Any

import numpy as np

__all__ = ["KINDS", "SUPPORTED_VERSION", "forward", "inverse", "plane_dtype", "validate_config"]

SUPPORTED_VERSION = "0.1"
KINDS = ("delta", "haar", "lift53")


def check_version(version: str) -> None:
    """Refuse configuration versions this implementation does not pin.

    The version is a *string* in the Zarr metadata, and Rust's ``NdLiftConfig``
    types it as one, so a numeric ``0.1`` is refused rather than stringified —
    otherwise the JSON number ``0.1`` would decode here but fail serde in the
    Rust codec. This is the single choke point every entry point calls.
    """
    if not isinstance(version, str):
        raise ValueError(
            f"nd_lift configuration version must be a string, got "
            f"{type(version).__name__} {version!r}"
        )
    parts = version.split(".")
    ok = len(parts) >= 2 and parts[0] == "0" and parts[1] == "1"
    if not ok:
        raise ValueError(
            f"nd_lift configuration version {version!r} is not supported by this build "
            f"(implements {SUPPORTED_VERSION}); refusing rather than mis-decoding"
        )


# The bounds the Rust ``AxisTransform`` field types impose. ``dimension`` is a
# ``usize`` bounded by the chunk rank, which is range-checked separately.
_INT_FIELD_MAX = {"dimension": None, "levels": 0xFF, "group": 0xFFFF_FFFF}
_INT_FIELD_RUST = {"dimension": "usize", "levels": "u8", "group": "u32"}


def _require_int(t: Mapping[str, Any], field: str, default: int = 0) -> int:
    """An integer-typed transform field, refusing anything ``int()`` would coerce.

    The Rust ``AxisTransform`` types these as ``usize``/``u8``/``u32``, so serde
    rejects a float, a string, a bool, or an out-of-range value outright. Bare
    ``int()`` here would instead *silently truncate* — ``levels: 2.9`` becoming
    2 changes the decomposition depth without telling anyone — so the type and
    the width are both checked.
    """
    value = t.get(field, default)
    # `bool` is an `int` subclass in Python; `True` is not a level count.
    if isinstance(value, bool) or not isinstance(value, numbers.Integral):
        raise ValueError(
            f"nd_lift transform {field} must be an integer, got "
            f"{type(value).__name__} {value!r}"
        )
    value = int(value)
    maximum = _INT_FIELD_MAX[field]
    if maximum is not None and not 0 <= value <= maximum:
        raise ValueError(
            f"nd_lift transform {field} {value} must be >= 0 and <= {maximum} "
            f"({_INT_FIELD_RUST[field]})"
        )
    return value


def validate_config(transforms: list[dict[str, Any]], ndim: int, version: str = "0.1") -> None:
    """Mirror the Rust codec's configuration validation.

    Every rejection is a ``ValueError``: a malformed configuration is an
    argument error, never a ``KeyError`` leaking out of a lookup or a value
    silently coerced into something the Rust codec would have refused.
    """
    check_version(version)
    allowed = {"axis", "dimension", "kind", "levels", "group"}
    for t in transforms:
        if not isinstance(t, Mapping):
            raise ValueError(
                f"nd_lift transform must be a mapping, got {type(t).__name__} {t!r}"
            )
        unknown = set(t) - allowed
        if unknown:
            raise ValueError(f"nd_lift transform has unknown fields {sorted(unknown)}")
        for required in ("kind", "dimension"):
            if required not in t:
                raise ValueError(f"nd_lift transform is missing required field {required!r}")
        kind = t["kind"]
        if kind not in KINDS:
            raise ValueError(f"nd_lift transform kind {kind!r} is not one of {KINDS}")
        # `axis` is a `String` in the Rust `AxisTransform` and the TypeScript
        # constructor checks the same thing, so a non-string axis has to fail
        # here too — otherwise a config Python writes is one serde refuses.
        if "axis" in t and not isinstance(t["axis"], str):
            raise ValueError(
                f"nd_lift transform axis must be a string, got "
                f"{type(t['axis']).__name__} {t['axis']!r}"
            )
        if not 0 <= _require_int(t, "dimension") < ndim:
            raise ValueError(
                f"nd_lift transform dimension {t['dimension']} out of range for a "
                f"{ndim}-dimensional chunk"
            )
        if kind in ("haar", "lift53") and _require_int(t, "levels") < 1:
            raise ValueError(f"nd_lift transform kind {kind!r} needs levels >= 1")
        # `group` is a `u32` in the Rust codec (0 = whole extent), so the width
        # check inside `_require_int` is what rejects a negative value. That
        # matters beyond tidiness: a negative group would make the segment
        # stride negative and skip the transform entirely, storing raw samples
        # that decode as a no-op too.
        _require_int(t, "group")


def plane_dtype(dtype: np.dtype) -> np.dtype:
    """The widened coefficient plane for an input dtype."""
    dtype = np.dtype(dtype)
    if dtype.kind not in "iu":
        raise ValueError(f"nd_lift supports integer dtypes, got {dtype}")
    return np.dtype(np.int32) if dtype.itemsize <= 4 else np.dtype(np.int64)


# ---------------------------------------------------------------------------
# 1D kernels over (n, m) planes: axis 0 is the transformed axis.
# ---------------------------------------------------------------------------
def _delta_forward(x: np.ndarray) -> np.ndarray:
    out = x.copy()
    out[1:] = x[1:] - x[:-1]
    return out


def _delta_inverse(x: np.ndarray) -> np.ndarray:
    return np.cumsum(x, axis=0, dtype=x.dtype)


def _haar_forward_level(x: np.ndarray) -> np.ndarray:
    n = x.shape[0]
    nd = n // 2
    x0 = x[0 : 2 * nd : 2]
    d = x[1 : 2 * nd : 2] - x0
    s = x0 + d // 2
    parts = [s, x[n - 1 : n], d] if n % 2 else [s, d]
    return np.concatenate(parts, axis=0)


def _haar_inverse_level(x: np.ndarray) -> np.ndarray:
    n = x.shape[0]
    ns = (n + 1) // 2
    nd = n // 2
    s, d = x[:nd], x[ns:]
    x0 = s - d // 2
    out = np.empty_like(x)
    out[0 : 2 * nd : 2] = x0
    out[1 : 2 * nd : 2] = x0 + d
    if n % 2:
        out[n - 1] = x[ns - 1]
    return out


def _mirrored_right(evens: np.ndarray, n: int) -> np.ndarray:
    """The right even neighbour of every odd sample, whole-sample mirrored."""
    if n % 2:
        return evens[1:]
    return np.concatenate([evens[1:], evens[-1:]], axis=0)


def _detail_neighbours(d: np.ndarray, ns: int, nd: int) -> tuple[np.ndarray, np.ndarray]:
    """``d[i-1]``/``d[i]`` for every even sample (``d[-1]=d[0]``, ``d[nd]=d[nd-1]``)."""
    idx = np.arange(ns)
    return d[np.maximum(idx - 1, 0)], d[np.minimum(idx, nd - 1)]


def _lift53_forward_level(x: np.ndarray) -> np.ndarray:
    n = x.shape[0]
    ns = (n + 1) // 2
    nd = n // 2
    evens = x[0::2]
    odds = x[1::2]
    d = odds - (evens[:nd] + _mirrored_right(evens, n) + 1) // 2
    dl, dr = _detail_neighbours(d, ns, nd)
    s = evens + (dl + dr + 2) // 4
    return np.concatenate([s, d], axis=0)


def _lift53_inverse_level(x: np.ndarray) -> np.ndarray:
    n = x.shape[0]
    ns = (n + 1) // 2
    nd = n // 2
    s, d = x[:ns], x[ns:]
    dl, dr = _detail_neighbours(d, ns, nd)
    evens = s - (dl + dr + 2) // 4
    odds = d + (evens[:nd] + _mirrored_right(evens, n) + 1) // 2
    out = np.empty_like(x)
    out[0::2] = evens
    out[1::2] = odds
    return out


def _band_lengths(n: int, levels: int) -> list[int]:
    bands = []
    m = n
    for _ in range(levels):
        if m < 2:
            break
        bands.append(m)
        m = (m + 1) // 2
    return bands


def _segment_forward(seg: np.ndarray, kind: str, levels: int) -> np.ndarray:
    if kind == "delta":
        return _delta_forward(seg)
    level_fn = _haar_forward_level if kind == "haar" else _lift53_forward_level
    out = seg.copy()
    for m in _band_lengths(seg.shape[0], levels):
        out[:m] = level_fn(out[:m])
    return out


def _segment_inverse(seg: np.ndarray, kind: str, levels: int) -> np.ndarray:
    if kind == "delta":
        return _delta_inverse(seg)
    level_fn = _haar_inverse_level if kind == "haar" else _lift53_inverse_level
    out = seg.copy()
    for m in reversed(_band_lengths(seg.shape[0], levels)):
        out[:m] = level_fn(out[:m])
    return out


def _apply_axis(chunk: np.ndarray, t: dict[str, Any], fwd: bool) -> np.ndarray:
    dim = int(t["dimension"])
    n = chunk.shape[dim]
    if n < 2:
        return chunk  # singleton axes are a no-op
    kind = t["kind"]
    levels = int(t.get("levels", 0))
    group = int(t.get("group", 0))
    g = n if group == 0 else min(group, n)
    moved = np.moveaxis(chunk, dim, 0)
    flat = np.ascontiguousarray(moved).reshape(n, -1)
    apply = _segment_forward if fwd else _segment_inverse
    for start in range(0, n, g):
        flat[start : start + g] = apply(flat[start : start + g], kind, levels)
    return np.moveaxis(flat.reshape(moved.shape), 0, dim)


# ---------------------------------------------------------------------------
# Overflow budget (mirrors ndic-lift's interval propagation exactly)
# ---------------------------------------------------------------------------
def _check_budget(chunk: np.ndarray, transforms: list[dict[str, Any]], plane: np.dtype) -> None:
    if chunk.size == 0:
        return
    plane_info = np.iinfo(plane)
    lo, hi = int(chunk.min()), int(chunk.max())
    # The plane-range check is *not* conditional on there being transforms:
    # widening is the codec's first act either way, and `uint32`/`uint64`
    # samples above the signed plane's maximum would otherwise wrap silently
    # in the `astype` that follows. The Rust codec refuses the same input.
    if lo < plane_info.min or hi > plane_info.max:
        raise OverflowError(
            f"nd_lift overflow budget: input range [{lo}, {hi}] does not fit the widened "
            f"{plane} coefficient plane"
        )
    if not transforms:
        return

    def fits(a: int, b: int) -> bool:
        return a >= plane_info.min and b <= plane_info.max

    for t in transforms:
        n = chunk.shape[int(t["dimension"])]
        if n < 2:
            continue
        group = int(t.get("group", 0))
        g = n if group == 0 else min(group, n)
        kind = t["kind"]
        levels = 1 if kind == "delta" else len(_band_lengths(g, int(t.get("levels", 0))))
        band_lo, band_hi = lo, hi
        for _ in range(levels):
            d_lo, d_hi = band_lo - band_hi, band_hi - band_lo
            if kind == "delta":
                s_lo, s_hi = lo, hi
            elif kind == "haar":
                s_lo = band_lo + d_lo // 2
                s_hi = band_hi + d_hi // 2
            else:  # lift53: the sums are intermediates that must fit too
                if not fits(2 * band_lo + 1, 2 * band_hi + 1):
                    break
                if not fits(2 * d_lo + 2, 2 * d_hi + 2):
                    break
                s_lo = band_lo + (2 * d_lo + 2) // 4
                s_hi = band_hi + (2 * d_hi + 2) // 4
            lo, hi = min(lo, d_lo, s_lo), max(hi, d_hi, s_hi)
            if not fits(lo, hi):
                break
            band_lo, band_hi = s_lo, s_hi
        else:
            continue
        raise OverflowError(
            f"nd_lift overflow budget exceeded on axis {t.get('axis', '')!r} "
            f"({t.get('levels', 0)} levels of {kind!r}): coefficients would leave the "
            f"{plane} plane; reduce levels or shorten the group"
        )


# ---------------------------------------------------------------------------
# Public chunk transforms
# ---------------------------------------------------------------------------
def forward(chunk: np.ndarray, transforms: list[dict[str, Any]], version: str = "0.1") -> np.ndarray:
    """Forward-transform ``chunk``, returning the widened coefficient plane."""
    validate_config(transforms, chunk.ndim, version)
    plane = plane_dtype(chunk.dtype)
    _check_budget(chunk, transforms, plane)
    out = chunk.astype(plane, copy=True)
    for t in transforms:
        out = _apply_axis(out, t, fwd=True)
    return np.ascontiguousarray(out)


def inverse(
    coeffs: np.ndarray,
    transforms: list[dict[str, Any]],
    dtype: np.dtype,
    version: str = "0.1",
) -> np.ndarray:
    """Inverse-transform a coefficient plane back to ``dtype`` samples."""
    validate_config(transforms, coeffs.ndim, version)
    dtype = np.dtype(dtype)
    if coeffs.dtype != plane_dtype(dtype):
        raise ValueError(
            f"nd_lift decode: expected a {plane_dtype(dtype)} coefficient plane for "
            f"{dtype} output, got {coeffs.dtype}"
        )
    out = coeffs.copy()
    for t in reversed(transforms):
        out = _apply_axis(out, t, fwd=False)
    info = np.iinfo(dtype)
    if out.size and (out.min() < info.min or out.max() > info.max):
        raise OverflowError(
            "nd_lift decode: coefficients do not narrow back to the array data type "
            "(corrupt or mismatched chunk)"
        )
    return np.ascontiguousarray(out.astype(dtype))
