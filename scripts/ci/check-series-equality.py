#!/usr/bin/env python3
"""Cross-language `codec_series` equality check.

Runs the shared fixture matrix through the Rust CLI (`ndic series`), the pure-Python
builder (`nd_image_codecs.codec_series`), and the pure-TypeScript builder
(`codecSeries`, via `npx tsx`), asserting byte-identical JSON pipelines.

Usage: python3 scripts/ci/check-series-equality.py
Requires: cargo, python3, and node/npx on PATH (tsx is fetched by npx).
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[2]
PY_PKG = REPO / "bindings" / "python" / "nd-image-codecs" / "python"
TS_SRC = REPO / "bindings" / "typescript" / "src" / "index.ts"

sys.path.insert(0, str(PY_PKG))
from nd_image_codecs import codec_series  # noqa: E402

# (axes, chunk_shape, dtype, family)
CASES = [
    (["t", "c", "z", "y", "x"], [8, 1, 32, 256, 256], "uint16", "nd-lift-ht"),
    (["t", "c", "z", "y", "x"], [1, 1, 32, 256, 256], "uint16", "nd-lift-ht"),
    (["t", "c", "z", "y", "x"], [8, 1, 32, 256, 256], "uint16", "nd-delta"),
    (["t", "c", "z", "y", "x"], [1, 3, 1, 512, 512], "uint8", "nd-delta"),
    (["x", "y", "z"], [256, 256, 32], "float32", "nd-zfp"),
    (["z", "y", "x"], [16, 128, 128], "int16", "nd-lift-ht"),
]


def rust_series(axes: list[str], chunks: list[int], dtype: str, family: str) -> object:
    out = subprocess.run(
        [
            "cargo", "run", "--quiet", "-p", "ndic-cli", "--", "series",
            "--axes", ",".join(axes),
            "--chunks", ",".join(map(str, chunks)),
            "--dtype", dtype,
            "--family", family,
        ],
        cwd=REPO, check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def ts_series(cases: list[tuple]) -> list[object]:
    script = f"""
import {{ codecSeries }} from {json.dumps(str(TS_SRC))};
const cases = {json.dumps([(a, c, d, f) for a, c, d, f in cases])};
const out = cases.map(([axes, chunks, dtype, family]) =>
  codecSeries(axes, chunks, dtype, family));
console.log(JSON.stringify(out));
"""
    with tempfile.NamedTemporaryFile("w", suffix=".mts", delete=False) as fh:
        fh.write(script)
        path = fh.name
    out = subprocess.run(
        ["npx", "-y", "tsx", path], check=True, capture_output=True, text=True, cwd=REPO
    ).stdout
    return json.loads(out)


def main() -> int:
    failures = 0
    ts_all = ts_series(CASES)
    for i, (axes, chunks, dtype, family) in enumerate(CASES):
        rs = rust_series(axes, chunks, dtype, family)
        py = codec_series(axes, chunks, dtype, family)
        ts = ts_all[i]
        label = f"{''.join(axes)}/{'x'.join(map(str, chunks))}/{dtype}/{family}"
        if rs == py == ts:
            print(f"ok   {label}")
        else:
            failures += 1
            print(f"FAIL {label}")
            print("  rust:", json.dumps(rs, sort_keys=True))
            print("  py:  ", json.dumps(py, sort_keys=True))
            print("  ts:  ", json.dumps(ts, sort_keys=True))
    if failures:
        print(f"{failures}/{len(CASES)} cases diverged", file=sys.stderr)
        return 1
    print(f"all {len(CASES)} cases identical across rust/python/typescript")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
