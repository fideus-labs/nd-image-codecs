#!/usr/bin/env python3
"""Generate the shared `codec_series` cross-language fixture matrix.

Writes `fixtures/codec-series/matrix.json`: axis layouts × chunk shapes ×
dtypes × families (plus targeted option-override and error cases), each with
the expected Zarr v3 pipeline JSON. The expected output is produced by the
pure-Python builder; the Rust and TypeScript builders are checked against the
committed file by their own test suites and by
`scripts/ci/check-series-equality.py`, so a divergence in any implementation
fails CI.

Regenerate (only when the builder algorithm deliberately changes):

    python3 scripts/gen-series-fixtures.py
"""

from __future__ import annotations

import json
import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "bindings" / "python" / "nd-image-codecs" / "python"))
from nd_image_codecs import codec_series  # noqa: E402

OUT = REPO / "fixtures" / "codec-series" / "matrix.json"

# (layout, [chunk variants])
LAYOUTS: list[tuple[list[str], list[list[int]]]] = [
    (["t", "c", "z", "y", "x"], [[8, 1, 32, 256, 256], [1, 1, 32, 256, 256], [4, 3, 16, 128, 128], [1, 1, 1, 512, 512]]),
    (["c", "z", "y", "x"], [[1, 32, 256, 256], [3, 16, 256, 256]]),
    (["z", "y", "x"], [[32, 256, 256], [1, 256, 256], [16, 64, 64]]),
    (["y", "x"], [[512, 512]]),
    (["x", "y", "z"], [[256, 256, 32], [64, 64, 8]]),
    (["t", "y", "x"], [[16, 256, 256], [1, 256, 256]]),
    (["c", "t", "z", "y", "x"], [[1, 8, 32, 128, 128], [2, 4, 16, 128, 128]]),
    (["q", "z", "y", "x"], [[5, 16, 128, 128]]),
]

# dtype sweep per family (defaults-only cases)
FAMILY_DTYPES: dict[str, list[str]] = {
    "nd-delta": ["uint8", "uint16", "float32"],
    "nd-lift-ht": ["uint8", "uint16", "int16"],
    "nd-zfp": ["float32", "int16"],
}

# (suffix, axes, chunk_shape, dtype, family, options)
OPTION_CASES: list[tuple[str, list[str], list[int], str, str, dict]] = [
    ("decorr-c-only", ["t", "c", "z", "y", "x"], [1, 4, 32, 256, 256], "uint16", "nd-lift-ht",
     {"decorrelate": [1]}),
    ("decorr-add-c", ["t", "c", "z", "y", "x"], [8, 4, 32, 256, 256], "uint16", "nd-lift-ht",
     {"add_decorrelate": [1]}),
    ("decorr-remove-z", ["t", "c", "z", "y", "x"], [1, 1, 32, 256, 256], "uint16", "nd-lift-ht",
     {"remove_decorrelate": [2]}),
    ("lift-haar", ["z", "y", "x"], [32, 256, 256], "uint16", "nd-lift-ht", {"lift": "haar"}),
    ("lift-delta", ["z", "y", "x"], [32, 256, 256], "uint16", "nd-lift-ht", {"lift": "delta"}),
    ("lossy-float", ["z", "y", "x"], [32, 256, 256], "float32", "nd-lift-ht",
     {"reversible": False, "xy_levels": 3}),
    ("delta-lz4", ["t", "c", "z", "y", "x"], [8, 1, 32, 256, 256], "uint16", "nd-delta",
     {"delta_backend": "lz4"}),
    ("delta-decorr-t", ["t", "c", "z", "y", "x"], [8, 1, 32, 256, 256], "uint16", "nd-delta",
     {"decorrelate": [0]}),
    ("zfp-rate8", ["z", "y", "x"], [32, 256, 256], "float32", "nd-zfp", {"zfp_rate": 8.0}),
    ("zfp-rate6.5", ["x", "y", "z"], [256, 256, 32], "float32", "nd-zfp", {"zfp_rate": 6.5}),
]

# (suffix, axes, chunk_shape, dtype, family, options) — every implementation
# must reject these.
ERROR_CASES: list[tuple[str, list[str], list[int], str, str, dict]] = [
    ("no-x-axis", ["z", "y", "w"], [32, 256, 256], "uint16", "nd-lift-ht", {}),
    ("chunk-rank-mismatch", ["z", "y", "x"], [32, 256], "uint16", "nd-lift-ht", {}),
    ("float-reversible-lift", ["z", "y", "x"], [32, 256, 256], "float32", "nd-lift-ht", {}),
    ("unknown-dtype", ["z", "y", "x"], [32, 256, 256], "complex64", "nd-delta", {}),
    ("decorr-x-rejected", ["z", "y", "x"], [32, 256, 256], "uint16", "nd-lift-ht", {"decorrelate": [2]}),
    ("zfp-too-many-dims", ["t", "c", "z", "y", "x"], [8, 4, 32, 256, 256], "float32", "nd-zfp", {}),
]


def case_name(axes: list[str], chunks: list[int], dtype: str, family: str, suffix: str = "") -> str:
    base = f"{''.join(axes)}/{'x'.join(map(str, chunks))}/{dtype}/{family}"
    return f"{base}/{suffix}" if suffix else base


def main() -> int:
    cases: list[dict] = []
    for axes, chunk_variants in LAYOUTS:
        for chunks in chunk_variants:
            for family, dtypes in FAMILY_DTYPES.items():
                if family == "nd-zfp" and sum(1 for c in chunks if c > 1) > 4:
                    continue  # covered by the zfp-too-many-dims error case
                for dtype in dtypes:
                    expected = codec_series(axes, chunks, dtype, family)
                    cases.append({
                        "name": case_name(axes, chunks, dtype, family),
                        "axes": axes,
                        "chunk_shape": chunks,
                        "dtype": dtype,
                        "family": family,
                        "expected": expected,
                    })
    for suffix, axes, chunks, dtype, family, options in OPTION_CASES:
        expected = codec_series(axes, chunks, dtype, family, **options)
        cases.append({
            "name": case_name(axes, chunks, dtype, family, suffix),
            "axes": axes,
            "chunk_shape": chunks,
            "dtype": dtype,
            "family": family,
            "options": options,
            "expected": expected,
        })
    for suffix, axes, chunks, dtype, family, options in ERROR_CASES:
        try:
            codec_series(axes, chunks, dtype, family, **options)
        except ValueError:
            pass
        else:
            raise AssertionError(f"error case {suffix} did not raise")
        case: dict = {
            "name": case_name(axes, chunks, dtype, family, suffix),
            "axes": axes,
            "chunk_shape": chunks,
            "dtype": dtype,
            "family": family,
            "error": True,
        }
        if options:
            case["options"] = options
        cases.append(case)

    names = [c["name"] for c in cases]
    assert len(names) == len(set(names)), "case names must be unique"
    OUT.parent.mkdir(parents=True, exist_ok=True)
    doc = {
        "$comment": "Generated by scripts/gen-series-fixtures.py — do not edit by hand.",
        "cases": cases,
    }
    OUT.write_text(json.dumps(doc, indent=2) + "\n")
    n_err = sum(1 for c in cases if c.get("error"))
    print(f"wrote {OUT.relative_to(REPO)}: {len(cases)} cases ({n_err} error cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
