#!/usr/bin/env python3
"""Package-version lockstep check.

The four registries publish one version number, and it lives in seven
hand-edited places (see `docs/development/publishing.md`, "Where the version
lives"). A partial bump is silent: `cargo publish` succeeds against a stale
`[workspace.dependencies]` pin and the resulting crates are unresolvable on
crates.io, and an npm/PyPI package can go out carrying the previous version.
This asserts every location agrees with `[workspace.package] version`.

The codec configuration `version` fields (`nd_lift`'s `"0.1"` and friends) are
a different number entirely — they version the on-disk codec format, not the
package — and are deliberately untouched here.

Usage: python3 scripts/ci/check-package-versions.py
Requires: python3 only. No build, no network.
"""

from __future__ import annotations

import json
import pathlib
import re
import sys
import tomllib

REPO = pathlib.Path(__file__).resolve().parents[2]
PY_PKG = REPO / "bindings" / "python" / "nd-image-codecs"
TS_PKG = REPO / "bindings" / "typescript"
JS_PKG = REPO / "bindings" / "javascript"

# `__version__` is read from the native module at run time; this literal is the
# fallback for a source tree whose extension has not been built yet.
VERSION_FALLBACK = re.compile(r'^\s*__version__(?:\s*:\s*str)?\s*=\s*"([^"]+)"', re.M)


def workspace_version(manifest: dict) -> str:
    return manifest["workspace"]["package"]["version"]


def internal_dep_versions(manifest: dict) -> list[tuple[str, str]]:
    """(label, version) for each `[workspace.dependencies]` in-repo path dep.

    These carry both `path` and `version`; cargo strips `path` when packaging
    and publishes the `version` requirement, so a stale one breaks every
    downstream crate on crates.io.
    """
    found = []
    for name, spec in manifest["workspace"]["dependencies"].items():
        if not isinstance(spec, dict) or "path" not in spec:
            continue
        # A path dep without a `version` is intentional for unpublished
        # members (see the ndic-bench-core note in Cargo.toml); nothing to
        # check, and flagging it would fight that decision.
        if "version" in spec:
            found.append((f"Cargo.toml [workspace.dependencies] {name}", spec["version"]))
    return found


def member_versions(manifest: dict) -> list[tuple[str, str]]:
    """(label, version) for each workspace member that pins its own version.

    Members are expected to inherit via `version.workspace = true`; one that
    spells out a literal instead silently escapes the bump.
    """
    found = []
    for member in manifest["workspace"]["members"]:
        path = REPO / member / "Cargo.toml"
        pkg = tomllib.loads(path.read_text())["package"]
        version = pkg.get("version")
        # `version.workspace = true` parses as {"workspace": True}: inherited,
        # so it cannot drift.
        if isinstance(version, str):
            found.append((f"{member}/Cargo.toml [package] version", version))
    return found


def locked_versions(members: set[str]) -> list[tuple[str, str]]:
    """(label, version) for each workspace member as recorded in Cargo.lock.

    A lockfile left unrefreshed after a manual bump is the one failure `cargo
    check` would fix on the spot but a release runbook can walk straight past.
    """
    lock = tomllib.loads((REPO / "Cargo.lock").read_text())
    found = []
    for pkg in lock["package"]:
        if pkg["name"] in members:
            found.append((f"Cargo.lock [[package]] {pkg['name']}", pkg["version"]))
    missing = members - {pkg["name"] for pkg in lock["package"]}
    if missing:
        raise RuntimeError(f"Cargo.lock is missing workspace members: {sorted(missing)}")
    return found


def package_name(path: pathlib.Path) -> str:
    return tomllib.loads(path.read_text())["package"]["name"]


def main() -> int:
    manifest = tomllib.loads((REPO / "Cargo.toml").read_text())
    expected = workspace_version(manifest)
    members = {package_name(REPO / m / "Cargo.toml") for m in manifest["workspace"]["members"]}

    ts_lock = json.loads((TS_PKG / "package-lock.json").read_text())
    py_init = (PY_PKG / "python" / "nd_image_codecs" / "__init__.py").read_text()
    fallback = VERSION_FALLBACK.search(py_init)
    if fallback is None:
        raise RuntimeError("no __version__ = \"...\" fallback found in __init__.py")

    checks: list[tuple[str, str]] = [
        ("Cargo.toml [workspace.package] version", expected),
        *internal_dep_versions(manifest),
        *member_versions(manifest),
        *locked_versions(members),
        ("pyproject.toml [project] version",
         tomllib.loads((PY_PKG / "pyproject.toml").read_text())["project"]["version"]),
        ("nd_image_codecs/__init__.py __version__ fallback", fallback.group(1)),
        ("bindings/typescript/package.json version",
         json.loads((TS_PKG / "package.json").read_text())["version"]),
        # Both project-level fields; the rest of the lockfile pins third-party
        # packages at their own versions and must not be touched.
        ("bindings/typescript/package-lock.json version", ts_lock["version"]),
        ('bindings/typescript/package-lock.json packages[""].version',
         ts_lock["packages"][""]["version"]),
        ("bindings/javascript/package.json version",
         json.loads((JS_PKG / "package.json").read_text())["version"]),
    ]

    bad = [(label, got) for label, got in checks if got != expected]
    for label, got in checks:
        print(f"{'FAIL' if got != expected else 'ok  '} {got:<12} {label}")

    if bad:
        print(f"\n{len(bad)} location(s) disagree with the workspace version "
              f"{expected!r}:", file=sys.stderr)
        for label, got in bad:
            print(f"  {label}: {got!r}", file=sys.stderr)
        print("\nSee docs/development/publishing.md, \"Where the version lives\".",
              file=sys.stderr)
        return 1

    print(f"\nall {len(checks)} locations read {expected}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
