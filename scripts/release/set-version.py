#!/usr/bin/env python3
"""Write one version number into every place the release needs it.

The four registries publish a single version, and it is written out 23 times
across seven files in three ecosystems (see `docs/development/publishing.md`,
"Where the version lives"). This is the writer; `scripts/ci/check-package-versions.py`
is the independent reader that proves the write landed everywhere, and this
script runs it before exiting unless `--no-verify` says otherwise.

The release workflow (`.github/workflows/release.yml`) runs this with the
version parsed out of the `vX.Y.Z` tag, so the tag — not the state of the
committed manifests — is what decides the version in every published artifact.
Maintainers run it through `scripts/release/prepare-release.sh` before tagging,
so the two agree in the ordinary case.

Everything is edited textually rather than round-tripped through a TOML/JSON
writer. `Cargo.toml`, `Cargo.lock`, and `pyproject.toml` all carry comments and
deliberate formatting that a `tomllib`-plus-serializer pass would silently
reflow, and `Cargo.lock` is machine-generated but reviewed in diffs. Only the
version literals move.

Usage:

    python3 scripts/release/set-version.py 0.2.0
    python3 scripts/release/set-version.py --from-tag v0.2.0
    python3 scripts/release/set-version.py 0.2.0 --no-verify

Requires: python3 only. No build, no network, no cargo, no npm.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys
import tomllib

REPO = pathlib.Path(__file__).resolve().parents[2]
PY_PKG = REPO / "bindings" / "python" / "nd-image-codecs"
TS_PKG = REPO / "bindings" / "typescript"
JS_PKG = REPO / "bindings" / "javascript"
CHECKER = REPO / "scripts" / "ci" / "check-package-versions.py"

# Same grammar the release workflow's tag parser enforces: SemVer 2.0.0 without
# build metadata (`+…` is legal SemVer but neither crates.io nor PyPI accepts
# it in a version, so it can never be published and is rejected here too).
SEMVER = re.compile(
    r"^(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)"
    r"(?:-(?P<prerelease>(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?$"
)

# A `version = "…"` assignment at the top level of whatever section we are in.
VERSION_ASSIGN = re.compile(r'^(?P<lead>\s*version\s*=\s*")(?P<version>[^"]*)(?P<tail>".*)$')
# `[section]` / `[[array.of.tables]]` headers, which is all the section tracking
# below needs — these files never indent their headers.
SECTION = re.compile(r"^\[\[?(?P<name>[^\]]+)\]\]?\s*(?:#.*)?$")
# `__version__ = "0.1.0"`, with or without the `: str` annotation.
PY_FALLBACK = re.compile(r'^(?P<lead>\s*__version__(?:\s*:\s*str)?\s*=\s*")(?P<version>[^"]*)(?P<tail>".*)$')


class Edit(Exception):
    """A file did not contain what this script was written to rewrite."""


def read(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def write(path: pathlib.Path, text: str) -> None:
    path.write_text(text, encoding="utf-8", newline="")


def rewrite_cargo_manifest(path: pathlib.Path, version: str) -> list[str]:
    """Rewrite `[workspace.package] version`, the internal `[workspace.dependencies]`
    pins, and a literal `[package] version` — whichever of them the file has.

    The internal path deps carry both `path` and `version`. cargo strips `path`
    when packaging and publishes the `version` requirement, so a stale one makes
    every downstream crate unresolvable on crates.io; they have to move with the
    workspace version, not lag it.
    """
    changed: list[str] = []
    section = ""
    out: list[str] = []

    for line in read(path).splitlines(keepends=True):
        header = SECTION.match(line.rstrip("\r\n"))
        if header:
            section = header.group("name").strip()
            out.append(line)
            continue

        if section in ("workspace.package", "package"):
            match = VERSION_ASSIGN.match(line.rstrip("\r\n"))
            if match:
                changed.append(f"{path.relative_to(REPO)} [{section}] version")
                out.append(f"{match['lead']}{version}{match['tail']}\n")
                continue

        # `ndic-core = { path = "crates/ndic-core", version = "0.1.0" }` — an
        # inline table on one line, key order not assumed.
        if section == "workspace.dependencies" and "path" in line and "version" in line:
            name = line.split("=", 1)[0].strip()
            replaced, count = re.subn(r'(version\s*=\s*")[^"]*(")', rf"\g<1>{version}\g<2>", line, count=1)
            if count:
                changed.append(f"{path.relative_to(REPO)} [workspace.dependencies] {name}")
                out.append(replaced)
                continue

        out.append(line)

    write(path, "".join(out))
    return changed


def rewrite_cargo_lock(path: pathlib.Path, version: str, members: set[str]) -> list[str]:
    """Rewrite the `version` of every workspace member recorded in `Cargo.lock`.

    Cargo would do this itself on the next `cargo check`, but a release runbook
    can walk straight past an unrefreshed lockfile, and `cargo publish` fails on
    one. Rewriting it here keeps the script free of a Rust toolchain: the
    `[[package]]` blocks are machine-generated with `name` immediately above
    `version`, and the `dependencies` lists hold bare names (cargo only
    disambiguates with a version when two versions of one crate are in the
    graph, which cannot happen for a path dependency).
    """
    changed: list[str] = []
    out: list[str] = []
    name: str | None = None
    seen: set[str] = set()

    for line in read(path).splitlines(keepends=True):
        stripped = line.rstrip("\r\n")
        if stripped == "[[package]]":
            name = None
        elif stripped.startswith("name = "):
            name = stripped.split("=", 1)[1].strip().strip('"')
        elif name in members:
            match = VERSION_ASSIGN.match(stripped)
            if match:
                changed.append(f"Cargo.lock [[package]] {name}")
                seen.add(name)
                out.append(f"{match['lead']}{version}{match['tail']}\n")
                name = None
                continue
        out.append(line)

    missing = members - seen
    if missing:
        raise Edit(f"Cargo.lock has no [[package]] entry for: {sorted(missing)}")

    write(path, "".join(out))
    return changed


def rewrite_pyproject(path: pathlib.Path, version: str) -> list[str]:
    section = ""
    out: list[str] = []
    changed: list[str] = []

    for line in read(path).splitlines(keepends=True):
        header = SECTION.match(line.rstrip("\r\n"))
        if header:
            section = header.group("name").strip()
        elif section == "project":
            match = VERSION_ASSIGN.match(line.rstrip("\r\n"))
            if match:
                changed.append(f"{path.relative_to(REPO)} [project] version")
                out.append(f"{match['lead']}{version}{match['tail']}\n")
                continue
        out.append(line)

    if not changed:
        raise Edit(f"{path.relative_to(REPO)}: no [project] version found")
    write(path, "".join(out))
    return changed


def rewrite_python_fallback(path: pathlib.Path, version: str) -> list[str]:
    """Rewrite the `__version__` literal the package falls back to.

    At run time `__version__` is read from the compiled extension; this literal
    only answers for a source tree whose extension has not been built. It still
    has to move, or an editable install reports the previous release.
    """
    out: list[str] = []
    changed: list[str] = []

    for line in read(path).splitlines(keepends=True):
        match = PY_FALLBACK.match(line.rstrip("\r\n"))
        if match and not changed:
            changed.append(f"{path.relative_to(REPO)} __version__ fallback")
            out.append(f"{match['lead']}{version}{match['tail']}\n")
            continue
        out.append(line)

    if not changed:
        raise Edit(f"{path.relative_to(REPO)}: no __version__ = \"…\" fallback found")
    write(path, "".join(out))
    return changed


def rewrite_package_json(path: pathlib.Path, version: str) -> list[str]:
    """Rewrite the project `version` in a `package.json` or `package-lock.json`.

    Parsed and re-serialized rather than patched line-wise, because a lockfile's
    two *project* fields sit among ~40 third-party `"version"` keys that must
    not move. `npm` writes these with two-space indent and a trailing newline,
    which `json.dumps` reproduces byte-for-byte — asserted by the round-trip
    check below rather than assumed, so an npm format change is a loud failure
    instead of a whole-file diff.
    """
    original = read(path)
    document = json.loads(original)
    if json.dumps(document, indent=2, ensure_ascii=False) + "\n" != original:
        raise Edit(
            f"{path.relative_to(REPO)} does not round-trip through json.dumps "
            "(indent=2 + trailing newline); rewriting it would reformat the whole file"
        )

    changed = [f"{path.relative_to(REPO)} version"]
    document["version"] = version
    # package-lock.json repeats the project version inside `packages[""]`; npm
    # itself keeps the two in step and `npm ci` warns when they disagree.
    root = document.get("packages", {}).get("")
    if isinstance(root, dict) and "version" in root:
        root["version"] = version
        changed.append(f'{path.relative_to(REPO)} packages[""].version')

    write(path, json.dumps(document, indent=2, ensure_ascii=False) + "\n")
    return changed


def workspace_members(manifest: dict) -> set[str]:
    """Crate names of every workspace member, read from their own manifests."""
    names = set()
    for member in manifest["workspace"]["members"]:
        names.add(tomllib.loads(read(REPO / member / "Cargo.toml"))["package"]["name"])
    return names


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("version", nargs="?", help="the release version, e.g. 0.2.0")
    source.add_argument("--from-tag", metavar="TAG", help="a release tag, e.g. v0.2.0")
    parser.add_argument(
        "--no-verify",
        action="store_true",
        help="skip the scripts/ci/check-package-versions.py confirmation pass",
    )
    args = parser.parse_args()

    version = args.version
    if args.from_tag is not None:
        if not args.from_tag.startswith("v"):
            print(f"tag {args.from_tag!r} does not start with 'v'", file=sys.stderr)
            return 2
        version = args.from_tag[1:]

    if not SEMVER.match(version):
        print(
            f"{version!r} is not a publishable SemVer version "
            "(MAJOR.MINOR.PATCH with an optional -prerelease, no +build metadata)",
            file=sys.stderr,
        )
        return 2

    manifest = tomllib.loads(read(REPO / "Cargo.toml"))
    members = workspace_members(manifest)

    changed: list[str] = []
    try:
        changed += rewrite_cargo_manifest(REPO / "Cargo.toml", version)
        for member in manifest["workspace"]["members"]:
            changed += rewrite_cargo_manifest(REPO / member / "Cargo.toml", version)
        changed += rewrite_cargo_lock(REPO / "Cargo.lock", version, members)
        changed += rewrite_pyproject(PY_PKG / "pyproject.toml", version)
        changed += rewrite_python_fallback(
            PY_PKG / "python" / "nd_image_codecs" / "__init__.py", version
        )
        changed += rewrite_package_json(TS_PKG / "package.json", version)
        changed += rewrite_package_json(TS_PKG / "package-lock.json", version)
        changed += rewrite_package_json(JS_PKG / "package.json", version)
    except Edit as error:
        print(f"set-version.py: {error}", file=sys.stderr)
        print(
            "\nThe layout this script rewrites has moved. Fix it here and in "
            "scripts/ci/check-package-versions.py together.",
            file=sys.stderr,
        )
        return 1

    for label in changed:
        print(f"set {version:<12} {label}")
    print(f"\nwrote {version} to {len(changed)} locations")

    if args.no_verify:
        return 0

    # The independent reader. It parses each file from scratch and knows the
    # same ten locations, so a location this script forgot to write — or a new
    # one added to the checker — fails here rather than at `cargo publish`.
    # Flushed first: the child writes to the same fd, and without this its
    # report lands ahead of the block above whenever stdout is a pipe.
    print()
    sys.stdout.flush()
    return subprocess.run([sys.executable, str(CHECKER)], check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
