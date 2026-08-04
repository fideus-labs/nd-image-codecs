#!/usr/bin/env python3
"""Execute every code block in ``docs/usage/*.md``.

The usage-docs gate. A snippet on the documentation site is a promise
about the current API; this runs each one so the promise is checked rather
than asserted.

**Every block runs.** A block opts out only with an HTML comment naming a
reason immediately above its fence::

    <!-- docs-check: skip — needs a GPU -->

which the report prints, so an exemption is visible in review rather than
silent. The comment is invisible both on GitHub and on the rendered site, so
the page reads the same for a human either way.

Per page, blocks of one language are concatenated in document order into a
single program and run once, which is what lets a later snippet use a name an
earlier one introduced — the way a reader works through the page. How each
language runs:

``bash``
    One script per page in a scratch directory, ``set -euo pipefail``, with
    the built ``ndic`` on ``PATH`` and a free port in ``DOCS_HTTP_PORT``.
    Pages that demonstrate HTTP Range start ``scripts/range-server.py``
    themselves — the checker deliberately runs no server of its own, so CI
    exercises exactly the command the documentation shows.
``rust``
    Concatenated into ``fn main() -> Result<(), Box<dyn Error>>`` inside a
    generated crate with path dependencies on the workspace, then built and
    run by cargo. (``use`` is legal inside a function body, so the snippets
    keep their imports where a reader expects them.)
``python``
    One module per page run with this interpreter.
``typescript``
    One module per page run with ``tsx`` from ``bindings/typescript``, so the
    package and zarrita resolve.
``json``
    Parsed, then — when it names a codec — round-tripped through that codec's
    Rust config parser via ``ndic``, so a documented configuration cannot
    drift from what the codec accepts.
``toml``
    Parsed; dependency tables additionally checked against the workspace's
    real crate versions.
``text``
    Never executed: the convention in this repository is that ``text`` marks
    ASCII diagrams and literal strings that are not code.

Usage: check-usage-docs.py [--only PAGE] [--keep] [--list]
"""

from __future__ import annotations

import argparse
import http.server
import json
import pathlib
import re
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
import tomllib

REPO = pathlib.Path(__file__).resolve().parents[2]
USAGE = REPO / "docs" / "usage"
TS_DIR = REPO / "bindings" / "typescript"

#: Languages this script knows how to run. `text` is deliberately absent.
EXECUTABLE = {"bash", "rust", "python", "typescript", "json", "toml", "shell"}

FENCE = re.compile(r"^```(\w+)\s*$")
DIRECTIVE = re.compile(r"^<!--\s*docs-check:\s*(?P<body>.*?)\s*-->\s*$")


class Block:
    def __init__(self, page: str, lang: str, line: int, code: str, skip: str | None):
        self.page = page
        self.lang = lang
        self.line = line
        self.code = code
        self.skip = skip

    def __str__(self) -> str:
        return f"{self.page}:{self.line} ({self.lang})"


def parse_blocks(path: pathlib.Path) -> list[Block]:
    """Every fenced block in a page, with any preceding docs-check directive."""
    blocks: list[Block] = []
    lines = path.read_text().splitlines()
    pending: str | None = None
    i = 0
    while i < len(lines):
        directive = DIRECTIVE.match(lines[i])
        if directive:
            body = directive.group("body")
            if body.startswith("skip"):
                pending = body[len("skip") :].lstrip(" —-:") or "no reason given"
            else:
                raise SystemExit(f"{path}:{i + 1}: unknown docs-check directive {body!r}")
            i += 1
            continue
        fence = FENCE.match(lines[i])
        if fence:
            lang = fence.group(1)
            start = i + 1
            end = start
            while end < len(lines) and lines[end].rstrip() != "```":
                end += 1
            code = "\n".join(lines[start:end])
            blocks.append(Block(path.name, lang, start, code, pending))
            pending = None
            i = end + 1
            continue
        if lines[i].strip() and pending is not None:
            raise SystemExit(
                f"{path}:{i + 1}: a docs-check directive must sit directly above a fence"
            )
        i += 1
    return blocks


def free_port() -> int:
    """A port that is free right now, for the docs' own server to bind."""
    with socketserver.TCPServer(("127.0.0.1", 0), http.server.BaseHTTPRequestHandler) as probe:
        return probe.server_address[1]


def build_ndic() -> pathlib.Path:
    out = subprocess.run(
        # `--features zarr` so the documented `ndic zarr` commands run too.
        ["cargo", "build", "-p", "ndic-cli", "--features", "zarr",
         "--message-format=json"],
        cwd=REPO, check=True, capture_output=True, text=True,
    ).stdout
    for line in out.splitlines():
        msg = json.loads(line)
        if msg.get("reason") == "compiler-artifact" and msg.get("executable") and \
                msg["target"]["name"] == "ndic":
            return pathlib.Path(msg["executable"])
    raise RuntimeError("cargo build did not report the ndic executable")


def run(argv: list[str], cwd: pathlib.Path, env: dict | None = None) -> None:
    proc = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, env=env)
    if proc.returncode != 0:
        raise RuntimeError(
            f"{' '.join(map(str, argv))} exited {proc.returncode}\n"
            f"--- stdout ---\n{proc.stdout}\n--- stderr ---\n{proc.stderr}"
        )


def run_bash(blocks: list[Block], workdir: pathlib.Path, ndic: pathlib.Path) -> None:
    script = workdir / "_docs_check.sh"
    body = "\n\n".join(b.code for b in blocks)
    script.write_text(f"set -euo pipefail\n\n{body}\n")
    env = {
        **__import__("os").environ,
        "PATH": f"{ndic.parent}:{__import__('os').environ['PATH']}",
        # The Range examples start scripts/range-server.py themselves; CI
        # only supplies a port that is free to bind.
        "DOCS_HTTP_PORT": str(free_port()),
    }
    run(["bash", str(script)], workdir, env)


RUST_CRATE_MANIFEST = """\
[package]
name = "usage-docs-{slug}"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
ndic-core = {{ path = "{repo}/crates/ndic-core" }}
ndic-codestream = {{ path = "{repo}/crates/ndic-codestream" }}
ndic-lift = {{ path = "{repo}/crates/ndic-lift", features = ["serde"] }}
ndic-zfp = {{ path = "{repo}/crates/ndic-zfp" }}
ndic-zarr = {{ path = "{repo}/crates/ndic-zarr", features = ["zarrs"] }}
serde_json = "1"

[workspace]
"""


def run_rust(blocks: list[Block], workdir: pathlib.Path) -> None:
    slug = blocks[0].page.removesuffix(".md").replace("-", "_")
    crate = workdir / "rust"
    (crate / "src").mkdir(parents=True)
    (crate / "Cargo.toml").write_text(
        RUST_CRATE_MANIFEST.format(slug=slug, repo=REPO.as_posix())
    )
    body = "\n\n".join(b.code for b in blocks)
    indented = "\n".join(f"    {line}" if line.strip() else "" for line in body.splitlines())
    (crate / "src" / "main.rs").write_text(
        "// Generated by scripts/ci/check-usage-docs.py from "
        f"docs/usage/{blocks[0].page}. Do not edit.\n"
        "fn main() -> Result<(), Box<dyn std::error::Error>> {\n"
        f"{indented}\n"
        "    Ok(())\n"
        "}\n"
    )
    run(["cargo", "run", "--quiet"], crate)


def run_python(blocks: list[Block], workdir: pathlib.Path) -> None:
    module = workdir / "usage_docs.py"
    module.write_text("\n\n".join(b.code for b in blocks) + "\n")
    run([sys.executable, str(module)], workdir)


def run_typescript(blocks: list[Block], workdir: pathlib.Path) -> None:
    # Runs from bindings/typescript so the package, zarrita, and tsx resolve;
    # the scratch directory is passed through for any file the snippets write.
    module = TS_DIR / "scripts" / f"_docs_check_{blocks[0].page.removesuffix('.md')}.mts"
    module.write_text("\n\n".join(b.code for b in blocks) + "\n")
    try:
        env = {**__import__("os").environ, "DOCS_WORKDIR": str(workdir)}
        run(["npx", "tsx", str(module)], TS_DIR, env)
    finally:
        module.unlink(missing_ok=True)


def run_json(block: Block, ndic: pathlib.Path) -> None:
    value = json.loads(block.code)
    name = value.get("name") if isinstance(value, dict) else None
    if name is None:
        return
    # A documented codec object must be one the codec actually accepts:
    # `ndic series --validate-codec` parses it with that codec's own config
    # type (serde, deny_unknown_fields) and re-emits it.
    proc = subprocess.run(
        [str(ndic), "series", "--validate-codec", json.dumps(value)],
        cwd=REPO, capture_output=True, text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"{name} configuration rejected by the codec:\n{proc.stderr}")
    got = json.loads(proc.stdout)
    if got != value:
        raise RuntimeError(
            f"{name} configuration does not round-trip through the codec:\n"
            f"  documented: {json.dumps(value, sort_keys=True)}\n"
            f"  codec says: {json.dumps(got, sort_keys=True)}"
        )


def run_toml(block: Block) -> None:
    parsed = tomllib.loads(block.code)
    deps = parsed.get("dependencies", {})
    if not deps:
        return
    workspace = tomllib.loads((REPO / "Cargo.toml").read_text())
    version = workspace["workspace"]["package"]["version"]
    for crate, spec in deps.items():
        if not (REPO / "crates" / crate).is_dir():
            raise RuntimeError(f"documented dependency {crate!r} is not a crate in this workspace")
        documented = spec if isinstance(spec, str) else spec.get("version", "")
        # Documented specs are the released-series prefix (e.g. "0.0" for
        # 0.0.2), which is what a user pins; a mismatch means the docs point
        # at a series this workspace no longer publishes.
        if not version.startswith(documented.lstrip("^~=")):
            raise RuntimeError(
                f"{crate}: docs say version {documented!r}, workspace is {version!r}"
            )


def check_page(path: pathlib.Path, ndic: pathlib.Path, tmp: pathlib.Path) -> list[str]:
    """Run one page's blocks; return failure messages."""
    failures: list[str] = []
    blocks = parse_blocks(path)
    workdir = tmp / path.stem
    workdir.mkdir(parents=True, exist_ok=True)

    for block in blocks:
        if block.lang not in EXECUTABLE and block.lang != "text" and block.skip is None:
            failures.append(f"{block}: unknown language {block.lang!r}")

    runnable = [b for b in blocks if b.skip is None and b.lang in EXECUTABLE]
    grouped: dict[str, list[Block]] = {}
    for block in runnable:
        grouped.setdefault(block.lang, []).append(block)

    # Per-block languages first (a bad configuration should not be reported as
    # a page-wide failure), then the concatenated program languages.
    for block in grouped.pop("json", []):
        try:
            run_json(block, ndic)
        except Exception as err:  # noqa: BLE001 - reported, not raised
            failures.append(f"{block}: {err}")
    for block in grouped.pop("toml", []):
        try:
            run_toml(block)
        except Exception as err:  # noqa: BLE001
            failures.append(f"{block}: {err}")
    # `shell` blocks are $-prompt transcripts, not scripts: their commands are
    # covered by the page's bash blocks, so only the JSON output they show is
    # checked for being parseable.
    for block in grouped.pop("shell", []):
        body = block.code
        if "{" in body:
            try:
                json.loads(body[body.index("{"):].rsplit("}", 1)[0] + "}")
            except Exception as err:  # noqa: BLE001
                failures.append(f"{block}: transcript output is not valid JSON: {err}")

    runners = {
        "bash": lambda bs: run_bash(bs, workdir, ndic),
        "rust": lambda bs: run_rust(bs, workdir),
        "python": lambda bs: run_python(bs, workdir),
        "typescript": lambda bs: run_typescript(bs, workdir),
    }
    for lang, blocks_of_lang in grouped.items():
        try:
            runners[lang](blocks_of_lang)
        except Exception as err:  # noqa: BLE001
            lines = ", ".join(str(b.line) for b in blocks_of_lang)
            failures.append(f"{path.name} {lang} blocks (lines {lines}): {err}")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", help="run one page (file name or stem)")
    parser.add_argument("--keep", action="store_true", help="keep the scratch directory")
    parser.add_argument("--list", action="store_true", help="list blocks and exit")
    args = parser.parse_args()

    pages = sorted(USAGE.glob("*.md"))
    if args.only:
        stem = args.only.removesuffix(".md")
        pages = [p for p in pages if p.stem == stem]
        if not pages:
            print(f"no usage page named {args.only!r}", file=sys.stderr)
            return 1

    if args.list:
        for page in pages:
            for block in parse_blocks(page):
                state = f"skip: {block.skip}" if block.skip else (
                    "run" if block.lang in EXECUTABLE else "not executable"
                )
                print(f"{block}  [{state}]")
        return 0

    print("building ndic…", flush=True)
    ndic = build_ndic()
    # Under target/ (not the system tmpdir) so repository-relative commands
    # in the snippets — `git rev-parse --show-toplevel` above all — resolve.
    (REPO / "target").mkdir(exist_ok=True)
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="usage-docs-", dir=REPO / "target"))
    failed = 0
    skipped: list[Block] = []
    try:
        for page in pages:
            skipped += [b for b in parse_blocks(page) if b.skip]
            failures = check_page(page, ndic, tmp)
            print(f"  {page.name:<32} {'ok' if not failures else 'FAIL'}", flush=True)
            for failure in failures:
                print(f"    - {failure}", file=sys.stderr)
            failed += bool(failures)
    finally:
        if args.keep:
            print(f"scratch directory kept: {tmp}")
        else:
            shutil.rmtree(tmp, ignore_errors=True)

    if skipped:
        print(f"\n{len(skipped)} block(s) exempted by an explicit docs-check directive:")
        for block in skipped:
            print(f"  {block}: {block.skip}")
    print(f"\n{len(pages) - failed}/{len(pages)} usage pages green")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
