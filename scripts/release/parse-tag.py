#!/usr/bin/env python3
"""Turn a release tag into the facts the release workflow branches on.

`.github/workflows/release.yml` fires on `v*`, which matches a great deal more
than a release — `v2-experiment`, `vendor-bump`, a typo'd `v0.2`. This is the
gate: a tag that does not parse as `v<SemVer>` stops the workflow before any
registry is contacted, and everything downstream reads the version from here
rather than re-deriving it from `github.ref_name`.

Build metadata (`1.2.3+build`) is rejected even though SemVer allows it: cargo
accepts it, PyPI has no representation for it, and npm silently drops it — so a
tag carrying it could never publish the same version to all four registries.

Written to `$GITHUB_OUTPUT` when that variable is set, and to stdout either way:

    version     0.2.0          the bare version, for every manifest
    tag         v0.2.0         the tag as pushed
    prerelease  true|false     a `-rc.1`-style suffix is present
    npm-tag     latest|next    the npm dist-tag to publish under

The npm dist-tag matters more than the others: publishing a prerelease without
`--tag next` makes `npm install <pkg>` resolve to it for everyone.

Usage:

    python3 scripts/release/parse-tag.py v0.2.0
    python3 scripts/release/parse-tag.py "$GITHUB_REF_NAME"

Requires: python3 only. No build, no network.
"""

from __future__ import annotations

import argparse
import os
import pathlib
import re
import sys

# SemVer 2.0.0 minus the `+build` suffix; see the module docstring. Kept
# character-for-character in step with `SEMVER` in set-version.py.
SEMVER = re.compile(
    r"^(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)"
    r"(?:-(?P<prerelease>(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?$"
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("tag", help="the pushed tag, e.g. v0.2.0")
    args = parser.parse_args()

    tag = args.tag.strip()
    if not tag.startswith("v"):
        print(f"tag {tag!r} does not start with 'v'", file=sys.stderr)
        return 2

    version = tag[1:]
    match = SEMVER.match(version)
    if match is None:
        print(
            f"tag {tag!r} is not a release tag: expected v<MAJOR>.<MINOR>.<PATCH> "
            "with an optional -prerelease and no +build metadata",
            file=sys.stderr,
        )
        return 2

    prerelease = match["prerelease"] is not None
    outputs = {
        "version": version,
        "tag": tag,
        "prerelease": "true" if prerelease else "false",
        "npm-tag": "next" if prerelease else "latest",
    }

    for key, value in outputs.items():
        print(f"{key:<11} {value}")

    github_output = os.environ.get("GITHUB_OUTPUT")
    if github_output:
        # No value here can contain a newline — SemVer forbids it and the regex
        # is anchored — so the plain `key=value` form is safe without a
        # heredoc delimiter.
        with pathlib.Path(github_output).open("a", encoding="utf-8") as handle:
            for key, value in outputs.items():
                handle.write(f"{key}={value}\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
