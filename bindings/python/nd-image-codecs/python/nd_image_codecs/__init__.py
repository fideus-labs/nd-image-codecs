"""nd-image-codecs — a family of composable Zarr v3 codecs for ND scientific images.

nd-image-codecs contributes three codec **families**, each a series (pipeline) of
Zarr v3 codecs assembled by :func:`codec_series` from an array's axis metadata:

- **nd-delta** — ``transpose → numcodecs.delta → bitshuffle → zstd/lz4``; fast
  lossless storage built entirely from existing Zarr codecs.
- **nd-lift-ht** — ``transpose → nd_lift → htj2k``; a cross-axis lifting
  transform feeding HTJ2K coefficient planes for scalable microscopy and
  volume visualization.
- **nd-zfp** — ``transpose → nd_zfp``; ZFP blocks with a brick index for GPU
  volume rendering, random access, and predictable memory.

:class:`NdLift` is implemented (roadmap Phase 2): its transform math lives in
:mod:`nd_image_codecs._lift` (the NumPy port of the Rust ``ndic-lift`` crate,
pinned bit-identical by the committed conformance vectors) and
:mod:`nd_image_codecs.zarr_codec` registers it as a ``zarr-python`` v3
array-to-array codec. ``htj2k`` is implemented (roadmap Phase 4) as
``zarr_codec.Htj2kCodec``, backed by the native extension module — the same
Rust core as the ``zarrs`` codec, so both ecosystems produce byte-identical
chunks; the :class:`Htj2k` class here is its plain configuration mirror.
:class:`NdZfp` is a scaffold until its roadmap phase lands; see
``docs/development/roadmap/``.
:func:`codec_series` is fully implemented in pure Python and is cross-checked
against the Rust and TypeScript builders in CI.

No component of nd-image-codecs uses JPEG 2000 Part 2 (MCT) syntax; cross-axis
decorrelation is expressed explicitly as the ``nd_lift`` array-to-array codec.
"""

from __future__ import annotations

from typing import Any, Literal

__all__ = [
    "Htj2k",
    "NdLift",
    "NdZfp",
    "__version__",
    "codec_series",
]

try:
    from ._nd_image_codecs import version as _native_version

    __version__: str = _native_version()
except ImportError:  # pragma: no cover - native module not built yet
    __version__ = "0.0.2"


# --------------------------------------------------------------------------
# Registered codec classes (scaffolds; see roadmap)
# --------------------------------------------------------------------------
class NdLift:
    """Zarr v3 array-to-array cross-axis lifting codec (``nd_lift``).

    This config class serializes exactly the configurations the Rust codec
    accepts (version ``0.1``: ``delta``/``haar``/``lift53`` transforms with
    ``dimension``, ``levels``, and ``group``). :meth:`encode`/:meth:`decode`
    run the NumPy reference transform; for ``zarr-python`` pipelines use
    :mod:`nd_image_codecs.zarr_codec`, which registers the same math as a
    zarr v3 codec.
    """

    name = "nd_lift"

    def __init__(self, *, transforms: list[dict[str, Any]] | None = None, version: str = "0.1") -> None:
        from . import _lift

        self.transforms = transforms or []
        self.version = version
        _lift.check_version(version)

    def to_dict(self) -> dict[str, Any]:
        """Return the Zarr v3 codec metadata object."""
        return {"name": self.name, "configuration": {"version": self.version, "transforms": self.transforms}}

    @classmethod
    def from_config(cls, config: dict[str, Any]) -> NdLift:
        """Construct from a Zarr v3 ``configuration`` object.

        ``version`` is required here, matching the Rust ``NdLiftConfig``
        (which has no serde default for it) and the TypeScript class: a
        configuration object read back from storage states its own semantics
        rather than inheriting whichever version this build implements. The
        constructor's default is for authoring configurations in code.
        """
        from . import _lift

        if "version" not in config:
            raise ValueError(
                'nd_lift configuration must carry an explicit "version" '
                f"(this build implements {_lift.SUPPORTED_VERSION}); "
                "refusing rather than mis-decoding"
            )
        return cls(transforms=config.get("transforms", []), version=config["version"])

    def encode(self, chunk: Any) -> Any:
        """Forward-transform an ndarray chunk into its widened coefficient plane."""
        from . import _lift

        return _lift.forward(chunk, self.transforms, self.version)

    def decode(self, coeffs: Any, dtype: Any) -> Any:
        """Inverse-transform a coefficient plane back to ``dtype`` samples."""
        from . import _lift

        return _lift.inverse(coeffs, self.transforms, dtype, self.version)


class Htj2k:
    """Zarr v3 array-to-bytes HTJ2K coefficient-plane codec (``htj2k``).

    Independent JPEG 2000 Part 1 / Part 15 codestreams per trailing 2D plane,
    plus a coefficient-plane byte index for range access. This class is the
    plain configuration object; the registered zarr-python codec (backed by
    the native extension) is :class:`nd_image_codecs.zarr_codec.Htj2kCodec`.
    """

    name = "htj2k"

    def __init__(
        self,
        *,
        xy_levels: int = 5,
        reversible: bool = True,
        progression: str = "RPCL",
        index: bool = True,
    ) -> None:
        self.xy_levels = xy_levels
        self.reversible = reversible
        self.progression = progression
        self.index = index

    def to_dict(self) -> dict[str, Any]:
        """Return the Zarr v3 codec metadata object."""
        return {
            "name": self.name,
            "configuration": {
                "xy_levels": self.xy_levels,
                "reversible": self.reversible,
                "progression": self.progression,
                "index": self.index,
            },
        }

    @classmethod
    def from_config(cls, config: dict[str, Any]) -> Htj2k:
        """Construct from a Zarr v3 ``configuration`` object."""
        return cls(
            xy_levels=config.get("xy_levels", 5),
            reversible=config.get("reversible", True),
            progression=config.get("progression", "RPCL"),
            index=config.get("index", True),
        )


class NdZfp:
    """Zarr v3 array-to-bytes ZFP codec (``nd_zfp``).

    This config class serializes exactly the configurations the Rust codec
    accepts: ``mode`` (``reversible``/``fixed_rate``/``fixed_accuracy``/
    ``fixed_precision``) with the corresponding parameter
    (``rate``/``tolerance``/``precision``) and the ZFP field
    dimensionality ``dims``. For ``zarr-python`` pipelines use
    :mod:`nd_image_codecs.zarr_codec`, which runs the native core.
    """

    name = "nd_zfp"

    def __init__(
        self,
        *,
        mode: str = "reversible",
        rate: float | None = None,
        tolerance: float | None = None,
        precision: int | None = None,
        dims: int = 3,
    ) -> None:
        self.mode = mode
        self.rate = rate
        self.tolerance = tolerance
        self.precision = precision
        self.dims = dims

    def to_dict(self) -> dict[str, Any]:
        """Return the Zarr v3 codec metadata object."""
        cfg: dict[str, Any] = {"mode": self.mode}
        if self.rate is not None:
            cfg["rate"] = self.rate
        if self.tolerance is not None:
            cfg["tolerance"] = self.tolerance
        if self.precision is not None:
            cfg["precision"] = self.precision
        cfg["dims"] = self.dims
        return {"name": self.name, "configuration": cfg}

    @classmethod
    def from_config(cls, config: dict[str, Any]) -> NdZfp:
        """Construct from a Zarr v3 ``configuration`` object."""
        return cls(
            mode=config.get("mode", "reversible"),
            rate=config.get("rate"),
            tolerance=config.get("tolerance"),
            precision=config.get("precision"),
            dims=config.get("dims", 3),
        )


# --------------------------------------------------------------------------
# codec_series — pure-Python mirror of ndic_zarr::series::codec_series
# --------------------------------------------------------------------------
Family = Literal["nd-delta", "nd-lift-ht", "nd-zfp"]

_DTYPES: dict[str, tuple[str, int]] = {
    "uint8": ("|u1", 1),
    "int8": ("|i1", 1),
    "uint16": ("<u2", 2),
    "int16": ("<i2", 2),
    "uint32": ("<u4", 4),
    "int32": ("<i4", 4),
    "uint64": ("<u8", 8),
    "int64": ("<i8", 8),
    "float32": ("<f4", 4),
    "float64": ("<f8", 8),
}


def codec_series(
    axes: list[str],
    chunk_shape: list[int],
    dtype: str,
    family: Family = "nd-lift-ht",
    *,
    decorrelate: list[int] | None = None,
    add_decorrelate: list[int] | None = None,
    remove_decorrelate: list[int] | None = None,
    lift: str = "lift53",
    xy_levels: int = 5,
    reversible: bool = True,
    delta_backend: Literal["zstd", "lz4"] = "zstd",
    zfp_rate: float | None = None,
) -> list[dict[str, Any]]:
    """Build a Zarr v3 codec pipeline for one nd-image-codecs family.

    This is a faithful port of the Rust ``ndic_zarr::series::codec_series``
    builder; CI asserts the three implementations agree byte-for-byte.

    Parameters
    ----------
    axes:
        Axis identifier per array dimension, in dimension order, e.g.
        ``["t", "c", "z", "y", "x"]``. ``"x"`` and ``"y"`` are required.
    chunk_shape:
        Chunk size per array dimension (same order as ``axes``).
    dtype:
        Zarr v3 data-type name, e.g. ``"uint16"``.
    family:
        ``"nd-delta"``, ``"nd-lift-ht"``, or ``"nd-zfp"``.
    decorrelate:
        Explicit dimension indices to decorrelate; overrides the defaults
        (``z``, and ``t`` when its chunk size is not 1).
    add_decorrelate, remove_decorrelate:
        Adjust the defaults instead of replacing them.
    """
    ndim = len(axes)
    if len(chunk_shape) != ndim:
        raise ValueError(f"{ndim} axes but chunk shape has {len(chunk_shape)} entries")
    idx = {name: i for i, name in enumerate(axes)}
    if len(idx) != ndim:
        raise ValueError("axis names must be unique")
    if "x" not in idx or "y" not in idx:
        raise ValueError("an 'x' and a 'y' axis are required")
    x, y = idx["x"], idx["y"]
    z = idx.get("z")
    t = idx.get("t")
    if dtype not in _DTYPES:
        raise ValueError(f"unsupported dtype {dtype!r}")
    np_dtype, itemsize = _DTYPES[dtype]
    if family == "nd-lift-ht" and "f" in np_dtype and reversible:
        raise ValueError(f"nd-lift-ht reversible coding needs an integer dtype, got {dtype!r}")

    # decorrelation set
    defaults = [d for d in (z, t) if d is not None and chunk_shape[d] > 1]
    if decorrelate is not None:
        decorr = list(decorrelate)
    else:
        decorr = list(defaults)
        for d in add_decorrelate or []:
            if d not in decorr:
                decorr.append(d)
        decorr = [d for d in decorr if d not in (remove_decorrelate or [])]
    for d in decorr:
        if d >= ndim:
            raise ValueError(f"invalid decorrelation dimension {d}")
        if d in (x, y):
            raise ValueError("the primary spatial axes (x, y) are decorrelated by the 2D codec itself")
    decorr = sorted(set(decorr))

    # target order: [ leading..., extra decorrelated..., t?, z?, y, x ]
    t_grouped = t is not None and chunk_shape[t] > 1 and t in decorr
    trailing: list[int] = []
    if t is not None and t_grouped:
        trailing.append(t)
    if z is not None:
        trailing.append(z)
    trailing += [y, x]
    extra = [d for d in decorr if d not in trailing]
    order = [d for d in range(ndim) if d not in trailing and d not in extra]
    order += extra + trailing

    if family == "nd-delta":
        a = next((d for d in (z, t) if d is not None and d in decorr), None)
        if a is not None:
            order = [d for d in order if d != a] + [a]

    codecs: list[dict[str, Any]] = []
    if order != list(range(ndim)):
        codecs.append({"name": "transpose", "configuration": {"order": order}})

    def pos_of(d: int) -> int:
        return order.index(d)

    if family == "nd-delta":
        codecs.append({"name": "numcodecs.delta", "configuration": {"dtype": np_dtype}})
        codecs.append({"name": "bytes", "configuration": {"endian": "little"}})
        codecs.append(
            {
                "name": "blosc",
                "configuration": {
                    "cname": delta_backend,
                    "clevel": 5,
                    "shuffle": "bitshuffle",
                    "typesize": itemsize,
                    "blocksize": 0,
                },
            }
        )
    elif family == "nd-lift-ht":
        transforms = [
            {
                "axis": axes[d],
                "dimension": pos_of(d),
                "kind": lift,
                "levels": 0 if lift == "delta" else 2,
                "group": 0,
            }
            for d in decorr
        ]
        if transforms:
            codecs.append({"name": "nd_lift", "configuration": {"version": "0.1", "transforms": transforms}})
        codecs.append(
            {
                "name": "htj2k",
                "configuration": {
                    "xy_levels": xy_levels,
                    "reversible": reversible,
                    "progression": "RPCL",
                    "index": True,
                },
            }
        )
    elif family == "nd-zfp":
        nonsingleton = sum(1 for c in chunk_shape if c > 1)
        if nonsingleton > 4:
            raise ValueError(f"nd-zfp needs <=4 non-singleton chunk dimensions, got {nonsingleton}")
        cfg: dict[str, Any] = {"mode": "reversible" if zfp_rate is None else "fixed_rate", "dims": max(nonsingleton, 2)}
        if zfp_rate is not None:
            cfg["rate"] = zfp_rate
        codecs.append({"name": "nd_zfp", "configuration": cfg})
    else:  # pragma: no cover - guarded by typing
        raise ValueError(f"unknown family {family!r}")

    return codecs
