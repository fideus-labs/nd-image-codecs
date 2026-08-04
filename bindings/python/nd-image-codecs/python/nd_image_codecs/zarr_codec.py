"""``nd_lift``, ``htj2k``, and ``nd_zfp`` as ``zarr-python`` (v3) codecs.

Importing this module requires ``zarr >= 3.1`` — the release that introduced
``zarr.core.dtype`` and the ``to_native_dtype()`` accessor these codecs
resolve dtypes through. The codecs are registered with zarr both via the
``zarr.codecs`` entry point (when this package is installed) and explicitly
through :func:`register`, so pipelines authored by
:func:`nd_image_codecs.codec_series` — the flagship Phase 4 series
``transpose → nd_lift → htj2k`` above all — run end-to-end:

>>> import nd_image_codecs.zarr_codec  # doctest: +SKIP
>>> nd_image_codecs.zarr_codec.register()  # doctest: +SKIP

The ``nd_lift`` transform math lives in :mod:`nd_image_codecs._lift`, the
NumPy port of the Rust ``ndic-lift`` crate; both are pinned bit-identical by
the committed conformance vectors (``fixtures/nd-lift/vectors.json``). The
``htj2k`` and ``nd_zfp`` codecs have no pure-Python port — they call the
native extension module (the same Rust cores as the ``zarrs`` codecs and
the WASM binding, so every ecosystem produces byte-identical chunks) and
raise a clear error when the package was installed without it.
"""

from __future__ import annotations

import asyncio
import math
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Self

import numpy as np
from zarr.abc.codec import ArrayArrayCodec, ArrayBytesCodec
from zarr.core.array_spec import ArraySpec
from zarr.core.common import JSON, parse_named_configuration
from zarr.core.dtype import Int32, Int64
from zarr.registry import register_codec

from . import _lift

if TYPE_CHECKING:
    from zarr.core.buffer import Buffer, NDBuffer

__all__ = ["Htj2kCodec", "NdLiftCodec", "NdZfpCodec", "ReshapeCodec", "register"]


@dataclass(frozen=True)
class NdLiftCodec(ArrayArrayCodec):
    """The ``nd_lift`` cross-axis integer lifting codec (version ``0.1``)."""

    is_fixed_size = True

    version: str = _lift.SUPPORTED_VERSION
    transforms: tuple[dict[str, Any], ...] = field(default_factory=tuple)

    def __init__(
        self,
        *,
        transforms: list[dict[str, Any]] | tuple[dict[str, Any], ...] = (),
        version: str = _lift.SUPPORTED_VERSION,
    ) -> None:
        _lift.check_version(version)
        object.__setattr__(self, "version", str(version))
        object.__setattr__(self, "transforms", tuple(dict(t) for t in transforms))

    def __hash__(self) -> int:
        # `transforms` holds dicts, so the frozen dataclass's generated
        # `__hash__` raises. zarr hashes codecs (e.g. the sharding codec's
        # cached pipeline lookups), so hash the same content `__eq__`
        # compares, in a normalized hashable form.
        return hash(
            (self.version, tuple(tuple(sorted(t.items())) for t in self.transforms)),
        )

    @classmethod
    def from_dict(cls, data: dict[str, JSON]) -> Self:
        _, configuration = parse_named_configuration(data, "nd_lift")
        # The Rust codec's `NdLiftConfig` has no serde default for `version`,
        # so a stored configuration without one is refused there. Refuse it
        # here too rather than assuming the version this build happens to
        # implement — the constructor's default is for authoring configs in
        # code, not for reading them back off storage.
        if "version" not in configuration:
            raise ValueError(
                'nd_lift configuration must carry an explicit "version" '
                f"(this build implements {_lift.SUPPORTED_VERSION}); "
                "refusing rather than mis-decoding"
            )
        return cls(**configuration)  # type: ignore[arg-type]

    def to_dict(self) -> dict[str, JSON]:
        return {
            "name": "nd_lift",
            "configuration": {
                "version": self.version,
                "transforms": [dict(t) for t in self.transforms],
            },
        }

    def validate(self, *, shape: tuple[int, ...], dtype: Any, chunk_grid: Any) -> None:
        _lift.validate_config(list(self.transforms), len(shape), self.version)
        _lift.plane_dtype(dtype.to_native_dtype())

    def resolve_metadata(self, chunk_spec: ArraySpec) -> ArraySpec:
        # The fill value only widens into the plane, matching the Rust codec's
        # `encoded_fill_value`. It is deliberately *not* `forward([v, v, ...])`:
        # for a non-zero `v` no scalar could be, since a constant chunk lifts
        # to something non-uniform (`[v, 0, ...]` under delta). What downstream
        # zarr actually asks of this value is symmetry, not that — an inner
        # chunk equal to it is dropped on write (`write_empty_chunks=False`)
        # and restored from the same `ArraySpec.fill_value` on read, so drop
        # and restore agree whatever the value is. Absent *outer* chunks never
        # reach this codec at all: zarr fills those in the decoded domain.
        # `test_zarr_pipeline_non_zero_fill_value` pins both paths.
        plane = _lift.plane_dtype(chunk_spec.dtype.to_native_dtype())
        return ArraySpec(
            shape=chunk_spec.shape,
            dtype=Int32() if plane == np.dtype(np.int32) else Int64(),
            fill_value=int(chunk_spec.fill_value),
            config=chunk_spec.config,
            prototype=chunk_spec.prototype,
        )

    async def _encode_single(self, chunk_array: NDBuffer, chunk_spec: ArraySpec) -> NDBuffer:
        data = np.ascontiguousarray(chunk_array.as_numpy_array())
        coeffs = _lift.forward(data, list(self.transforms), self.version)
        return chunk_spec.prototype.nd_buffer.from_numpy_array(coeffs)

    async def _decode_single(self, chunk_array: NDBuffer, chunk_spec: ArraySpec) -> NDBuffer:
        coeffs = np.ascontiguousarray(chunk_array.as_numpy_array())
        dtype = chunk_spec.dtype.to_native_dtype()
        data = _lift.inverse(coeffs, list(self.transforms), dtype, self.version)
        return chunk_spec.prototype.nd_buffer.from_numpy_array(data)

    def compute_encoded_size(self, input_byte_length: int, chunk_spec: ArraySpec) -> int:
        dtype = chunk_spec.dtype.to_native_dtype()
        plane = _lift.plane_dtype(dtype)
        return input_byte_length * plane.itemsize // dtype.itemsize


#: Zarr dtype names the htj2k integer path accepts.
_HTJ2K_DTYPES = frozenset({"uint8", "int8", "uint16", "int16", "uint32", "int32"})

#: Progression orders the codestream writer implements.
_HTJ2K_PROGRESSIONS = frozenset({"LRCP", "RLCP", "RPCL", "PCRL", "CPRL"})


def _native():
    """The native extension module, or a clear error when it is absent."""
    try:
        from . import _nd_image_codecs
    except ImportError as err:  # pragma: no cover - build-environment issue
        raise RuntimeError(
            "the htj2k and nd_zfp codecs need nd-image-codecs's native "
            "extension module; install a built wheel (or `maturin develop`) "
            "rather than the pure-Python source tree"
        ) from err
    return _nd_image_codecs


@dataclass(frozen=True)
class Htj2kCodec(ArrayBytesCodec):
    """The ``htj2k`` coefficient-plane codec: one HTJ2K codestream per
    trailing 2D plane plus the coefficient-plane byte index
    (``docs/architecture/range-access.md``)."""

    is_fixed_size = False

    xy_levels: int = 5
    reversible: bool = True
    progression: str = "RPCL"
    index: bool = True

    def __post_init__(self) -> None:
        if not 0 <= int(self.xy_levels) <= 32:
            raise ValueError("htj2k: xy_levels must be in 0..=32")
        if self.progression not in _HTJ2K_PROGRESSIONS:
            raise ValueError(f"htj2k: unknown progression order {self.progression!r}")

    @classmethod
    def from_dict(cls, data: dict[str, JSON]) -> Self:
        _, configuration = parse_named_configuration(
            data, "htj2k", require_configuration=False
        )
        return cls(**(configuration or {}))  # type: ignore[arg-type]

    def to_dict(self) -> dict[str, JSON]:
        return {
            "name": "htj2k",
            "configuration": {
                "xy_levels": self.xy_levels,
                "reversible": self.reversible,
                "progression": self.progression,
                "index": self.index,
            },
        }

    def validate(self, *, shape: tuple[int, ...], dtype: Any, chunk_grid: Any) -> None:
        name = dtype.to_native_dtype().name
        if name not in _HTJ2K_DTYPES:
            raise ValueError(
                f"htj2k has no integer path for dtype {name!r}; "
                "use nd-zfp for float data"
            )
        if len(shape) < 2:
            raise ValueError("htj2k needs a chunk with trailing 2D (y, x) planes")

    async def _encode_single(self, chunk_array: NDBuffer, chunk_spec: ArraySpec) -> Buffer:
        data = np.ascontiguousarray(chunk_array.as_numpy_array())
        # Off the event loop, like zarr's own Zstd/Blosc codecs: the native
        # call is CPU-bound (and releases the GIL), so `to_thread` keeps
        # chunk pipelining and other async I/O running during the encode.
        blob = await asyncio.to_thread(
            _native().htj2k_encode,
            data.tobytes(),
            list(data.shape),
            data.dtype.name,
            xy_levels=self.xy_levels,
            reversible=self.reversible,
            progression=self.progression,
            index=self.index,
        )
        return chunk_spec.prototype.buffer.from_bytes(blob)

    async def _decode_single(self, chunk_bytes: Buffer, chunk_spec: ArraySpec) -> NDBuffer:
        dtype = chunk_spec.dtype.to_native_dtype()
        raw = await asyncio.to_thread(
            _native().htj2k_decode,
            chunk_bytes.to_bytes(),
            list(chunk_spec.shape),
            dtype.name,
        )
        data = np.frombuffer(raw, dtype=dtype).reshape(chunk_spec.shape)
        return chunk_spec.prototype.nd_buffer.from_numpy_array(data)

    def compute_encoded_size(self, input_byte_length: int, chunk_spec: ArraySpec) -> int:
        raise NotImplementedError("htj2k chunks have no fixed encoded size")


#: Zarr dtype names the nd_zfp codec accepts: ZFP's native types plus the
#: narrow integers the core promotes to int32.
_ND_ZFP_DTYPES = frozenset(
    {"uint8", "int8", "uint16", "int16", "int32", "int64", "float32", "float64"}
)

#: nd_zfp compression modes and the parameter each one takes.
_ND_ZFP_MODES: dict[str, str | None] = {
    "reversible": None,
    "fixed_rate": "rate",
    "fixed_accuracy": "tolerance",
    "fixed_precision": "precision",
}


@dataclass(frozen=True)
class NdZfpCodec(ArrayBytesCodec):
    """The ``zfp`` codec: chunks compressed as 1-4D ZFP fields with the full
    ZFP header (``docs/architecture/zfp.md``). Also answers to the deprecated
    ``nd_zfp`` name, whose configurations carry a legacy ``dims`` selecting
    the old squeeze-and-pad chunk mapping."""

    is_fixed_size = False

    mode: str = "reversible"
    rate: float | None = None
    tolerance: float | None = None
    precision: int | None = None
    dims: int | None = None

    def __post_init__(self) -> None:
        if self.mode not in _ND_ZFP_MODES:
            raise ValueError(f"zfp: unknown mode {self.mode!r}")
        needed = _ND_ZFP_MODES[self.mode]
        for name in ("rate", "tolerance", "precision"):
            value = getattr(self, name)
            if name == needed and value is None:
                raise ValueError(f'zfp: {self.mode} mode needs a "{name}"')
            if name != needed and value is not None:
                raise ValueError(f"zfp: {self.mode!r} mode does not take {name!r}")
        # Mirror the Rust core's numeric bounds (`ZfpMode::validate`), so a
        # bad configuration fails here rather than inside the native call.
        if self.rate is not None and not (math.isfinite(self.rate) and self.rate > 0):
            raise ValueError(f"zfp: rate must be a positive finite number, got {self.rate}")
        if self.tolerance is not None and not (
            math.isfinite(self.tolerance) and self.tolerance >= 0
        ):
            raise ValueError(
                f"zfp: tolerance must be a non-negative finite number, got {self.tolerance}"
            )
        if self.precision is not None and not (
            isinstance(self.precision, int) and 1 <= self.precision <= 64
        ):
            raise ValueError(f"zfp: precision must be an integer in 1..=64, got {self.precision}")
        if self.dims is not None and not 1 <= int(self.dims) <= 4:
            raise ValueError("zfp: dims must be in 1..=4")

    @classmethod
    def from_dict(cls, data: dict[str, JSON]) -> Self:
        # `zfp` is the registered name; `nd_zfp` predates the adoption and
        # stays readable.
        name = data.get("name")
        if name not in ("zfp", "nd_zfp"):
            raise ValueError(f"expected the zfp codec (or its nd_zfp alias), got {name!r}")
        configuration = data.get("configuration") or {}
        return cls(**configuration)  # type: ignore[arg-type]

    def to_dict(self) -> dict[str, JSON]:
        configuration: dict[str, JSON] = {"mode": self.mode}
        for name in ("rate", "tolerance", "precision"):
            value = getattr(self, name)
            if value is not None:
                configuration[name] = value
        if self.dims is not None:
            configuration["dims"] = self.dims
        return {"name": "zfp", "configuration": configuration}

    def _mode_kwargs(self) -> dict[str, Any]:
        return {
            "mode": self.mode,
            "rate": self.rate,
            "tolerance": self.tolerance,
            "precision": self.precision,
            "dims": self.dims,
        }

    def validate(self, *, shape: tuple[int, ...], dtype: Any, chunk_grid: Any) -> None:
        name = dtype.to_native_dtype().name
        if name not in _ND_ZFP_DTYPES:
            raise ValueError(f"zfp has no path for dtype {name!r}")

    async def _encode_single(self, chunk_array: NDBuffer, chunk_spec: ArraySpec) -> Buffer:
        data = np.ascontiguousarray(chunk_array.as_numpy_array())
        # Off the event loop, like htj2k: the native call is CPU-bound and
        # releases the GIL.
        blob = await asyncio.to_thread(
            _native().nd_zfp_encode,
            data.tobytes(),
            list(data.shape),
            data.dtype.name,
            **self._mode_kwargs(),
        )
        return chunk_spec.prototype.buffer.from_bytes(blob)

    async def _decode_single(self, chunk_bytes: Buffer, chunk_spec: ArraySpec) -> NDBuffer:
        dtype = chunk_spec.dtype.to_native_dtype()
        raw = await asyncio.to_thread(
            _native().nd_zfp_decode,
            chunk_bytes.to_bytes(),
            list(chunk_spec.shape),
            dtype.name,
            **self._mode_kwargs(),
        )
        data = np.frombuffer(raw, dtype=dtype).reshape(chunk_spec.shape)
        return chunk_spec.prototype.nd_buffer.from_numpy_array(data)

    def compute_encoded_size(self, input_byte_length: int, chunk_spec: ArraySpec) -> int:
        raise NotImplementedError("zfp chunks have no fixed encoded size")


@dataclass(frozen=True)
class ReshapeCodec(ArrayArrayCodec):
    """The ``reshape`` codec (zarr-extensions): C-order-preserving reshape.

    Implements the subset of the specification the codec-series builder
    emits and validates the rest: each ``shape`` entry is a positive size, a
    strictly increasing list of input dimensions whose extents multiply into
    the output extent, or a single ``-1`` inferred from the remaining
    extents. The nd-zfp family uses it to collapse singleton chunk
    dimensions so the ``zfp`` codec receives a direct 1-4D field.
    """

    is_fixed_size = True

    shape: tuple[Any, ...] = ()

    def __init__(self, *, shape: list[Any] | tuple[Any, ...]) -> None:
        object.__setattr__(self, "shape", tuple(
            tuple(entry) if isinstance(entry, (list, tuple)) else entry for entry in shape
        ))

    @classmethod
    def from_dict(cls, data: dict[str, JSON]) -> Self:
        _, configuration = parse_named_configuration(data, "reshape")
        return cls(**(configuration or {}))  # type: ignore[arg-type]

    def to_dict(self) -> dict[str, JSON]:
        shape = [list(entry) if isinstance(entry, tuple) else entry for entry in self.shape]
        return {"name": "reshape", "configuration": {"shape": shape}}

    def _output_shape(self, input_shape: tuple[int, ...]) -> tuple[int, ...]:
        out: list[int] = []
        inferred_at: int | None = None
        used_dims: list[int] = []
        for i, entry in enumerate(self.shape):
            if isinstance(entry, tuple):
                if any(not 0 <= d < len(input_shape) for d in entry):
                    raise ValueError(f"reshape: input dims {entry} out of range for {input_shape}")
                used_dims.extend(entry)
                out.append(int(np.prod([input_shape[d] for d in entry], dtype=np.int64)))
            elif entry == -1:
                if inferred_at is not None:
                    raise ValueError("reshape: at most one -1 entry is allowed")
                inferred_at = i
                out.append(-1)
            elif isinstance(entry, int) and entry > 0:
                out.append(entry)
            else:
                raise ValueError(f"reshape: invalid shape entry {entry!r}")
        if used_dims != sorted(set(used_dims)):
            raise ValueError(f"reshape: input dims must be strictly increasing, got {used_dims}")
        total = int(np.prod(input_shape, dtype=np.int64))
        if inferred_at is not None:
            known = int(np.prod([d for d in out if d != -1], dtype=np.int64))
            if known == 0 or total % known:
                raise ValueError(f"reshape: cannot infer -1 for {input_shape} -> {out}")
            out[inferred_at] = total // known
        if int(np.prod(out, dtype=np.int64)) != total:
            raise ValueError(f"reshape: {input_shape} does not reshape to {out}")
        return tuple(out)

    def validate(self, *, shape: tuple[int, ...], dtype: Any, chunk_grid: Any) -> None:
        del dtype, chunk_grid

    def resolve_metadata(self, chunk_spec: ArraySpec) -> ArraySpec:
        return ArraySpec(
            shape=self._output_shape(chunk_spec.shape),
            dtype=chunk_spec.dtype,
            fill_value=chunk_spec.fill_value,
            config=chunk_spec.config,
            prototype=chunk_spec.prototype,
        )

    async def _encode_single(self, chunk_array: NDBuffer, chunk_spec: ArraySpec) -> NDBuffer:
        data = np.ascontiguousarray(chunk_array.as_numpy_array())
        return chunk_spec.prototype.nd_buffer.from_numpy_array(
            data.reshape(self._output_shape(data.shape))
        )

    async def _decode_single(self, chunk_array: NDBuffer, chunk_spec: ArraySpec) -> NDBuffer:
        data = np.ascontiguousarray(chunk_array.as_numpy_array())
        return chunk_spec.prototype.nd_buffer.from_numpy_array(
            data.reshape(chunk_spec.shape)
        )

    def compute_encoded_size(self, input_byte_length: int, chunk_spec: ArraySpec) -> int:
        return input_byte_length


def register() -> None:
    """Register the codecs with the zarr-python codec registry."""
    register_codec("nd_lift", NdLiftCodec)
    register_codec("htj2k", Htj2kCodec)
    register_codec("zfp", NdZfpCodec)
    register_codec("reshape", ReshapeCodec)
    # The pre-adoption name, kept readable for existing stores.
    register_codec("nd_zfp", NdZfpCodec)


register()
