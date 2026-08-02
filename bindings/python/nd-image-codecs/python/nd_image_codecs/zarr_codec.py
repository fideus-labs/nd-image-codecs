"""``nd_lift`` as a ``zarr-python`` (v3) array-to-array codec.

Importing this module requires ``zarr >= 3.1`` — the release that introduced
``zarr.core.dtype`` and the ``to_native_dtype()`` accessor this codec resolves
its coefficient plane through. The codec is registered with
zarr both via the ``zarr.codecs`` entry point (when this package is
installed) and explicitly through :func:`register`, so pipelines authored by
:func:`nd_image_codecs.codec_series` — e.g. the Phase 2 validation series
``transpose → nd_lift → bytes → blosc`` — run end-to-end:

>>> import nd_image_codecs.zarr_codec  # doctest: +SKIP
>>> nd_image_codecs.zarr_codec.register()  # doctest: +SKIP

The transform math lives in :mod:`nd_image_codecs._lift`, the NumPy port of
the Rust ``ndic-lift`` crate; both are pinned bit-identical by the committed
conformance vectors (``fixtures/nd-lift/vectors.json``).
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Self

import numpy as np
from zarr.abc.codec import ArrayArrayCodec
from zarr.core.array_spec import ArraySpec
from zarr.core.common import JSON, parse_named_configuration
from zarr.core.dtype import Int32, Int64
from zarr.registry import register_codec

from . import _lift

if TYPE_CHECKING:
    from zarr.core.buffer import NDBuffer

__all__ = ["NdLiftCodec", "register"]


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


def register() -> None:
    """Register the codec with the zarr-python codec registry."""
    register_codec("nd_lift", NdLiftCodec)


register()
