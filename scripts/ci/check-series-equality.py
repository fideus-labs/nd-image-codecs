#!/usr/bin/env python3
"""Cross-language `codec_series` equality check.

Runs the shared fixture matrix (`fixtures/codec-series/matrix.json`) through
the Rust CLI (`ndic series`), the pure-Python builder
(`nd_image_codecs.codec_series`), and the pure-TypeScript builder
(`codecSeries`, via `npx tsx`), asserting that all three produce JSON
identical to each other **and** to the committed expected pipelines. Error
cases must be rejected by all three.

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
MATRIX = REPO / "fixtures" / "codec-series" / "matrix.json"

sys.path.insert(0, str(PY_PKG))
from nd_image_codecs import codec_series  # noqa: E402

ERROR = {"__error__": True}


def build_ndic() -> pathlib.Path:
    """Build the `ndic` binary once and return its path."""
    out = subprocess.run(
        ["cargo", "build", "-p", "ndic-cli", "--message-format=json"],
        cwd=REPO, check=True, capture_output=True, text=True,
    ).stdout
    for line in out.splitlines():
        msg = json.loads(line)
        if msg.get("reason") == "compiler-artifact" and msg.get("executable") and \
                msg["target"]["name"] == "ndic":
            return pathlib.Path(msg["executable"])
    raise RuntimeError("cargo build did not report the ndic executable")


def series_args(case: dict) -> list[str]:
    """Map one fixture case onto `ndic series` CLI arguments."""
    args = [
        "series",
        "--axes", ",".join(case["axes"]),
        "--chunks", ",".join(map(str, case["chunk_shape"])),
        "--dtype", case["dtype"],
        "--family", case["family"],
    ]
    opts = case.get("options", {})
    for key, flag in [
        ("decorrelate", "--decorrelate"),
        ("add_decorrelate", "--add-decorrelate"),
        ("remove_decorrelate", "--remove-decorrelate"),
    ]:
        if key in opts:
            args += [flag, ",".join(map(str, opts[key]))]
    if "lift" in opts:
        args += ["--lift", opts["lift"]]
    if "xy_levels" in opts:
        args += ["--xy-levels", str(opts["xy_levels"])]
    if opts.get("reversible") is False:
        args.append("--lossy")
    if "delta_backend" in opts:
        args += ["--delta-backend", opts["delta_backend"]]
    if "zfp_rate" in opts:
        args += ["--zfp-rate", str(opts["zfp_rate"])]
    return args


def rust_series(ndic: pathlib.Path, case: dict) -> object:
    proc = subprocess.run(
        [str(ndic), *series_args(case)], cwd=REPO, capture_output=True, text=True,
    )
    if proc.returncode != 0:
        return ERROR
    return json.loads(proc.stdout)


def python_series(case: dict) -> object:
    try:
        return codec_series(
            case["axes"], case["chunk_shape"], case["dtype"], case["family"],
            **case.get("options", {}),
        )
    except ValueError:
        return ERROR


def ts_series(cases: list[dict]) -> list[object]:
    """Run every case through the TypeScript builder in one tsx invocation."""
    script = f"""
import {{ codecSeries }} from {json.dumps(str(TS_SRC))};
const cases = {json.dumps(cases)};
const toOpts = (raw = {{}}) => {{
  const o = {{}};
  if (raw.decorrelate !== undefined) o.decorrelate = raw.decorrelate;
  if (raw.add_decorrelate !== undefined) o.addDecorrelate = raw.add_decorrelate;
  if (raw.remove_decorrelate !== undefined) o.removeDecorrelate = raw.remove_decorrelate;
  if (raw.lift !== undefined) o.lift = raw.lift;
  if (raw.xy_levels !== undefined) o.xyLevels = raw.xy_levels;
  if (raw.reversible !== undefined) o.reversible = raw.reversible;
  if (raw.delta_backend !== undefined) o.deltaBackend = raw.delta_backend;
  if (raw.zfp_rate !== undefined) o.zfpRate = raw.zfp_rate;
  return o;
}};
const out = cases.map((c) => {{
  try {{
    return codecSeries(c.axes, c.chunk_shape, c.dtype, c.family, toOpts(c.options));
  }} catch {{
    return {{ __error__: true }};
  }}
}});
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
    cases = json.loads(MATRIX.read_text())["cases"]
    ndic = build_ndic()
    ts_all = ts_series(cases)
    failures = 0
    for case, ts in zip(cases, ts_all, strict=True):
        expected = ERROR if case.get("error") else case["expected"]
        results = {
            "rust": rust_series(ndic, case),
            "python": python_series(case),
            "typescript": ts,
        }
        bad = {lang: got for lang, got in results.items() if got != expected}
        if not bad:
            print(f"ok   {case['name']}")
        else:
            failures += 1
            print(f"FAIL {case['name']}")
            print("  expected:", json.dumps(expected, sort_keys=True))
            for lang, got in bad.items():
                print(f"  {lang}:", json.dumps(got, sort_keys=True))
    if failures:
        print(f"{failures}/{len(cases)} cases diverged", file=sys.stderr)
        return 1
    print(f"all {len(cases)} cases identical across rust/python/typescript")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
