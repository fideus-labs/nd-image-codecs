#!/usr/bin/env python3
"""The cross-ecosystem validation matrix.

For every case in ``fixtures/cross-validation/cases.json``, writes a Zarr v3
store with each of the three ecosystems and reads every store back with all
three:

- **zarrs** (Rust) via ``ndic zarr write|read`` (``--features zarr``),
- **zarr-python** via ``scripts/cross-validation/zarr_python_io.py`` (our
  entry-point codecs),
- **zarrita.js** via ``bindings/typescript/scripts/zarrita-io.mts`` (our WASM
  codec classes; needs ``npm run build:wasm`` first).

Every write x read direction pair must produce identical bytes; lossless
cases must additionally equal the generated input; and for the nd-lift-ht
and nd-zfp families — whose chunk encoders are one Rust core shared by all
three ecosystems — the stored chunk files themselves must be byte-identical
across writers. (nd-delta chunks go through three independent blosc builds,
so only their decoded bytes are compared.)

Requires: cargo, node 22 + the TypeScript workspace installed (``npm ci``)
with the WASM core built, and a Python with zarr>=3.1 + numpy + the
nd-image-codecs package importable (wheel or in-repo).

Usage: check-cross-validation.py [--only SUBSTRING] [--keep]
"""

from __future__ import annotations

import argparse
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import zlib

import numpy as np

REPO = pathlib.Path(__file__).resolve().parents[2]
CASES = REPO / "fixtures" / "cross-validation" / "cases.json"
TS_DIR = REPO / "bindings" / "typescript"
PY_IO = REPO / "scripts" / "cross-validation" / "zarr_python_io.py"

#: Value ranges per dtype, inside the nd_lift overflow budget and the HT
#: datapath's declared-depth ceiling for every family that codes integers.
INT_RANGES = {
    "uint8": (0, 200),
    "int8": (-100, 100),
    "uint16": (0, 4000),
    "int16": (-2000, 2000),
    "uint32": (0, 100_000),
    "int32": (-50_000, 50_000),
    "uint64": (0, 2**40),
    "int64": (-(2**39), 2**39),
}


def build_ndic() -> pathlib.Path:
    """Build ndic with the zarr feature and return the executable path."""
    out = subprocess.run(
        [
            "cargo", "build", "-p", "ndic-cli", "--features", "zarr",
            "--message-format=json",
        ],
        cwd=REPO, check=True, capture_output=True, text=True,
    ).stdout
    for line in out.splitlines():
        msg = json.loads(line)
        if msg.get("reason") == "compiler-artifact" and msg.get("executable") and \
                msg["target"]["name"] == "ndic":
            return pathlib.Path(msg["executable"])
    raise RuntimeError("cargo build did not report the ndic executable")


def generate(case: dict) -> bytes:
    """Deterministic little-endian input for a case, seeded by its name."""
    rng = np.random.default_rng(zlib.crc32(case["name"].encode()))
    dtype = np.dtype(case["dtype"]).newbyteorder("<")
    shape = tuple(case["shape"])
    if dtype.kind == "f":
        data = (rng.standard_normal(shape) * 100.0).astype(dtype)
    else:
        lo, hi = INT_RANGES[case["dtype"]]
        data = rng.integers(lo, hi, size=shape, dtype=dtype)
    return data.tobytes()


class Runner:
    """One ecosystem's write/read commands."""

    def __init__(self, name: str, write_argv, read_argv, cwd: pathlib.Path):
        self.name = name
        self.write_argv = write_argv
        self.read_argv = read_argv
        self.cwd = cwd

    def write(self, store: pathlib.Path, spec: pathlib.Path, input_: pathlib.Path) -> None:
        run(self.write_argv(store, spec, input_), self.cwd)

    def read(self, store: pathlib.Path, output: pathlib.Path) -> None:
        run(self.read_argv(store, output), self.cwd)


def run(argv: list[str], cwd: pathlib.Path) -> None:
    proc = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(
            f"command failed: {' '.join(map(str, argv))}\n"
            f"stdout: {proc.stdout}\nstderr: {proc.stderr}"
        )


def make_runners(ndic: pathlib.Path) -> list[Runner]:
    return [
        Runner(
            "zarrs",
            lambda s, spec, i: [str(ndic), "zarr", "write", "--store", str(s),
                                "--spec", str(spec), "--input", str(i)],
            lambda s, o: [str(ndic), "zarr", "read", "--store", str(s),
                          "--output", str(o)],
            REPO,
        ),
        Runner(
            "zarr-python",
            lambda s, spec, i: [sys.executable, str(PY_IO), "write", "--store", str(s),
                                "--spec", str(spec), "--input", str(i)],
            lambda s, o: [sys.executable, str(PY_IO), "read", "--store", str(s),
                          "--output", str(o)],
            REPO,
        ),
        Runner(
            "zarrita",
            lambda s, spec, i: ["npx", "tsx", "scripts/zarrita-io.mts", "write",
                                "--store", str(s), "--spec", str(spec), "--input", str(i)],
            lambda s, o: ["npx", "tsx", "scripts/zarrita-io.mts", "read",
                          "--store", str(s), "--output", str(o)],
            TS_DIR,
        ),
    ]


def chunk_files(store: pathlib.Path) -> dict[str, bytes]:
    return {
        str(p.relative_to(store)): p.read_bytes()
        for p in sorted(store.rglob("*"))
        if p.is_file() and p.name != "zarr.json"
    }


def is_lossless(case: dict) -> bool:
    opts = case.get("options", {})
    return opts.get("zfp_rate") is None and opts.get("reversible", True)


#: Families whose chunk encoder is the shared Rust core, so stored chunks
#: must be byte-identical across ecosystems.
BYTE_IDENTICAL_FAMILIES = {"nd-lift-ht", "nd-zfp"}


def check_case(case: dict, runners: list[Runner], workdir: pathlib.Path) -> list[str]:
    """Run one case through every direction pair; return failure messages."""
    failures: list[str] = []
    spec = workdir / "spec.json"
    spec.write_text(json.dumps(case))
    input_ = workdir / "input.raw"
    input_.write_bytes(generate(case))

    stores: dict[str, pathlib.Path] = {}
    for writer in runners:
        store = workdir / f"{writer.name}.zarr"
        try:
            writer.write(store, spec, input_)
            stores[writer.name] = store
        except RuntimeError as err:
            failures.append(f"{writer.name} write: {err}")

    outputs: dict[tuple[str, str], bytes] = {}
    for writer_name, store in stores.items():
        for reader in runners:
            out = workdir / f"{writer_name}--{reader.name}.raw"
            try:
                reader.read(store, out)
                outputs[(writer_name, reader.name)] = out.read_bytes()
            except RuntimeError as err:
                failures.append(f"{writer_name} -> {reader.name}: {err}")

    if outputs:
        reference_key = min(outputs)
        reference = outputs[reference_key]
        for key, got in outputs.items():
            if got != reference:
                failures.append(
                    f"{key[0]} -> {key[1]} disagrees with {reference_key[0]} -> {reference_key[1]}"
                )
        if is_lossless(case) and reference != input_.read_bytes():
            failures.append("lossless decode does not match the input")

    if case["family"] in BYTE_IDENTICAL_FAMILIES and len(stores) > 1:
        names = sorted(stores)
        chunks = {name: chunk_files(stores[name]) for name in names}
        first = names[0]
        for other in names[1:]:
            if set(chunks[first]) != set(chunks[other]):
                failures.append(f"chunk keys differ between {first} and {other}")
            else:
                diffs = [k for k in chunks[first] if chunks[first][k] != chunks[other][k]]
                if diffs:
                    failures.append(
                        f"stored chunks differ between {first} and {other}: {diffs[:4]}"
                    )
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", help="run only cases whose name contains this")
    parser.add_argument("--keep", action="store_true", help="keep the work directory")
    args = parser.parse_args()

    cases = json.loads(CASES.read_text())["cases"]
    if args.only:
        cases = [c for c in cases if args.only in c["name"]]
    if not cases:
        print("no cases selected", file=sys.stderr)
        return 1

    wasm = TS_DIR / "src" / "wasm" / "ndic_zarr_bg.wasm"
    if not wasm.is_file():
        print(f"missing {wasm}; run `npm run build:wasm` in bindings/typescript first",
              file=sys.stderr)
        return 1

    print("building ndic (--features zarr)…", flush=True)
    ndic = build_ndic()
    runners = make_runners(ndic)

    tmp = pathlib.Path(tempfile.mkdtemp(prefix="ndic-cross-validation-"))
    failed = 0
    try:
        for case in cases:
            workdir = tmp / case["name"]
            workdir.mkdir()
            failures = check_case(case, runners, workdir)
            status = "ok" if not failures else "FAIL"
            pairs = f"{len(runners)}x{len(runners)}"
            print(f"  {case['name']:<32} [{case['family']}] {pairs} {status}", flush=True)
            for failure in failures:
                print(f"    - {failure}", file=sys.stderr)
            failed += bool(failures)
    finally:
        if args.keep:
            print(f"work directory kept: {tmp}")
        else:
            shutil.rmtree(tmp, ignore_errors=True)

    total = len(cases)
    print(f"{total - failed}/{total} cases green "
          f"({len(runners)} writers x {len(runners)} readers each)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
