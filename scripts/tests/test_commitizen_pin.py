"""One Commitizen version, everywhere it is named.

Commitizen generates two things that have to agree: the `## vX.Y.Z` section
`cz changelog --incremental` writes into CHANGELOG.md, and the GitHub release
body `.github/workflows/release.yml` builds with `cz changelog <tag>`. Both read
`.cz.toml`, so the *configuration* cannot differ — but the generator reading it
can, and its output is not stable across versions: section grouping, the
`commit_parser` semantics, and the incremental boundary match have all moved
between Commitizen releases. A maintainer previewing a release with whatever
`uvx --from commitizen` resolves to today, then tagging, gets a release body
built by 4.17.0 and a CHANGELOG.md section built by something else.

That failure is quiet. Both commands succeed, both produce a plausible
changelog, and the difference only shows up as a released page that does not
match the file in the repository — after the tag is pushed, which is the point
past which none of it can be replaced.

So `scripts/release/prepare-release.sh` pins `CZ_VERSION`, and this asserts
every other place that names the package agrees with it: the workflow, and the
commands the documentation invites a reader to run. A pin may also *reference*
`CZ_VERSION` (`commitizen==${CZ_VERSION}`), which is how the release script
itself stays correct by construction.

This reads the real tree rather than a fixture, for the same reason the
workflow suite next door does: the files under test *are* the artifact.

Run with:

    uvx --with pytest --from pytest pytest scripts/tests -q

Requires: pytest. No network.
"""

from __future__ import annotations

import pathlib
import re
import shutil
import subprocess

import pytest

REPO = pathlib.Path(__file__).resolve().parents[2]
PREPARE = REPO / "scripts" / "release" / "prepare-release.sh"

# `CZ_VERSION="4.17.0"` in the release script — the one place the version is
# decided. Everything else in the repository either repeats it or interpolates
# it, and this suite is what makes repeating it safe.
CZ_VERSION_ASSIGNMENT = re.compile(r'^CZ_VERSION="(?P<version>[^"]+)"', re.MULTILINE)

# A package *specification*, not the word. `.cz.toml` and the publishing guide
# discuss Commitizen in prose at length, and none of that prose installs
# anything; what matters is the three flag forms that resolve a version — uv's
# `--from`, pipx's `--spec`, and an `install` argument — each immediately
# followed by the package name and an optional `==` pin.
#
# Deliberately no worked example here: a literal pinned command written in this
# file would be scanned like any other and would have to be maintained in step
# with CZ_VERSION, which is the exact chore this suite exists to remove.
#
# The optional flags between `install` and the package are why that arm is not
# a plain `install commitizen` — the release workflow passes one.
COMMITIZEN_SPEC = re.compile(
    r"(?:--from|--spec|install(?:\s+--[\w-]+)*)\s+commitizen(?:==(?P<version>[^\s\"'`)]+))?"
)

# Files a reader or a runner could take a command from. Excludes lockfiles and
# anything generated; `git ls-files` already excludes untracked build output.
TEXT_SUFFIXES = {".md", ".toml", ".yml", ".yaml", ".sh", ".py", ".rs", ".txt"}

# A regex that matches nothing passes every assertion below, so the scan is
# itself checked. This is a floor, not a census: it is deliberately well under
# the sites that exist, because raising it to the exact count would make every
# added or deleted documented command edit this line, and *removing* a command
# is not the defect this file exists to catch — naming a version other than
# CZ_VERSION is. `test_the_release_script_exercises_every_form` is the sharper
# half of the same guard: it pins the three shapes rather than the total.
MINIMUM_EXPECTED_SPECS = 8

# `prepare-release.sh` resolves commitizen three ways — uv, pipx run, and a
# pipx install hint — which is one instance of each form COMMITIZEN_SPEC knows.
# A single count over the whole tree cannot tell "one arm of the alternation
# broke" from "a documented command was deleted"; requiring all three from this
# one file can.
EXPECTED_FORMS_IN_PREPARE = 3


@pytest.fixture(scope="module")
def cz_version() -> str:
    match = CZ_VERSION_ASSIGNMENT.search(PREPARE.read_text(encoding="utf-8"))
    assert match, f"no CZ_VERSION assignment in {PREPARE.relative_to(REPO)}"
    return match["version"]


@pytest.fixture(scope="module")
def specs() -> list[tuple[pathlib.Path, int, str, str | None]]:
    """Every Commitizen package spec in the tracked tree.

    Each entry is (path, line number, the whole line, the pinned version or
    `None` when the spec carries no `==`).
    """
    # Resolved rather than spelled `"git"`, so the process starts from an
    # absolute path and a missing git says so plainly instead of surfacing as a
    # bare FileNotFoundError from inside the fixture. (This also satisfies
    # Ruff's S607, which flags a partial executable path.)
    git = shutil.which("git")
    assert git, "git is not on PATH, so the tracked file list cannot be read"

    tracked = subprocess.run(
        [git, "ls-files", "-z"],
        cwd=REPO,
        capture_output=True,
        check=True,
        text=True,
    ).stdout.split("\0")

    found = []
    for name in filter(None, tracked):
        path = REPO / name
        if path.suffix not in TEXT_SUFFIXES or not path.is_file():
            continue
        # `git ls-files` yields repository-relative names, so this is resolved
        # on both sides rather than compared as written.
        if path == pathlib.Path(__file__).resolve():
            continue
        for number, line in enumerate(
            path.read_text(encoding="utf-8", errors="replace").splitlines(), start=1
        ):
            for match in COMMITIZEN_SPEC.finditer(line):
                found.append((path, number, line.strip(), match["version"]))
    return found


def test_the_scan_finds_the_specs(specs):
    assert len(specs) >= MINIMUM_EXPECTED_SPECS, (
        f"only {len(specs)} Commitizen spec(s) found; the pattern has probably "
        "stopped matching a form the repository uses"
    )


def test_the_release_script_exercises_every_form(specs):
    """Each arm of the alternation still matches something.

    Without this, one arm could stop matching and the tree-wide floor above
    would absorb the loss — every command that arm covers would then go
    unchecked while the suite stayed green.
    """
    in_prepare = [spec for spec in specs if spec[0] == PREPARE]
    assert len(in_prepare) >= EXPECTED_FORMS_IN_PREPARE, (
        f"{PREPARE.relative_to(REPO)} resolves commitizen "
        f"{EXPECTED_FORMS_IN_PREPARE} ways, but the scan found "
        f"{len(in_prepare)}; an arm of COMMITIZEN_SPEC has stopped matching"
    )


def test_every_commitizen_spec_is_pinned(specs):
    unpinned = [
        f"{path.relative_to(REPO)}:{number}: {line}"
        for path, number, line, version in specs
        if version is None
    ]
    assert not unpinned, (
        "these install Commitizen at whatever version resolves today, so the "
        "changelog they generate need not match the released one:\n  "
        + "\n  ".join(unpinned)
    )


def test_every_pin_is_the_release_version(specs, cz_version):
    accepted = {cz_version, "${CZ_VERSION}", "$CZ_VERSION"}
    mismatched = [
        f"{path.relative_to(REPO)}:{number}: pins {version}, expected {cz_version}"
        for path, number, line, version in specs
        if version is not None and version not in accepted
    ]
    assert not mismatched, (
        f"CZ_VERSION is {cz_version} in {PREPARE.relative_to(REPO)}:\n  "
        + "\n  ".join(mismatched)
    )


def test_the_release_workflow_pins_the_same_version(cz_version):
    workflow = (REPO / ".github" / "workflows" / "release.yml").read_text(
        encoding="utf-8"
    )
    assert f"commitizen=={cz_version}" in workflow, (
        f"the release workflow does not install commitizen=={cz_version}; the "
        "GitHub release body would be generated by a different version than "
        "the one that wrote CHANGELOG.md"
    )
