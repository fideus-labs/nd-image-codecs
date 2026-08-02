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

from typing import Any

import numpy as np

__all__ = ["KINDS", "SUPPORTED_VERSION", "forward", "inverse", "plane_dtype", "validate_config"]

SUPPORTED_VERSION = "0.1"
KINDS = ("delta", "haar", "lift53")


def check_version(version: str) -> None:
    """Refuse configuration versions this implementation does not pin."""
    parts = str(version).split(".")
    ok = len(parts) >= 2 and parts[0] == "0" and parts[1] == "1"
    if not ok:
        raise ValueError(
            f"nd_lift configuration version {version!r} is not supported by this build "
            f"(implements {SUPPORTED_VERSION}); refusing rather than mis-decoding"
        )


def validate_config(transforms: list[dict[str, Any]], ndim: int, version: str = "0.1") -> None:
    """Mirror the Rust codec's configuration validation."""
    check_version(version)
    allowed = {"axis", "dimension", "kind", "levels", "group"}
    for t in transforms:
        unknown = set(t) - allowed
        if unknown:
            raise ValueError(f"nd_lift transform has unknown fields {sorted(unknown)}")
        kind = t["kind"]
        if kind not in KINDS:
            raise ValueError(f"nd_lift transform kind {kind!r} is not one of {KINDS}")
        if not 0 <= int(t["dimension"]) < ndim:
            raise ValueError(
                f"nd_lift transform dimension {t['dimension']} out of range for a "
                f"{ndim}-dimensional chunk"
            )
        if kind in ("haar", "lift53") and int(t.get("levels", 0)) < 1:
            raise ValueError(f"nd_lift transform kind {kind!r} needs levels >= 1")


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
    if chunk.size == 0 or not transforms:
        return
    plane_info = np.iinfo(plane)
    lo, hi = int(chunk.min()), int(chunk.max())
    if lo < plane_info.min or hi > plane_info.max:
        raise OverflowError(
            f"nd_lift overflow budget: input range [{lo}, {hi}] does not fit the widened "
            f"{plane} coefficient plane"
        )

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
