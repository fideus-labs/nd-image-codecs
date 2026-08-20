"""Tests for the *shape* of `.github/workflows/release.yml`.

The release scripts have their own suite next door. This one covers something
no script can hold: the `needs:` graph, which is what decides how many times a
release stops for a human and what has been proven before any of it becomes
irreversible.

Two properties live entirely in that graph, and both are invisible in review.
A `needs:` list is edited one job at a time, each edit looks local and correct,
and neither property announces that it broke — the first symptom of losing
either one is a release already halfway onto three registries.

1. **One approval.** GitHub asks for an environment reviewer once per set of
   jobs pending *at the same moment*. Four jobs enter `release`, so the number
   of prompts is a property of the graph, not of the environment settings. They
   all wait on `ready` and on nothing that `ready` does not already wait on, so
   they go pending together and one approval releases all four. Give any one of
   them an extra dependency and it arrives in its own wave — a second prompt,
   arriving after the first has already authorized the release, which is how an
   approval decays into something people click through.

2. **Nothing publishes until every artifact exists.** `crates-io` used to need
   only `verify`, so a wheel that failed to build stopped `pypi` *after* the
   seven crates were already on crates.io. Neither crates.io nor PyPI will
   reuse a version number, so that half-release could not be repaired, only
   abandoned. Everything that can fail harmlessly now runs before `ready`.

These read the real workflow file — there is no synthetic fixture worth
building here, because the file under test *is* the artifact.

Run with:

    uvx --with pytest --with pyyaml --from pytest pytest scripts/tests -q

Requires: pytest, PyYAML.
"""

from __future__ import annotations

import pathlib

import pytest
import yaml

REPO = pathlib.Path(__file__).resolve().parents[2]
WORKFLOW = REPO / ".github" / "workflows" / "release.yml"

# The gate every publishing job passes through, and the environment whose
# reviewer requirement is the manual approval.
GATE = "ready"
ENVIRONMENT = "release"

# Derived below rather than assumed, but named here so a job that quietly loses
# `environment: release` — which would take both its approval *and* its trusted
# publishing credential with it — fails as a mismatch rather than vanishing
# from every set this module checks.
EXPECTED_PUBLISHERS = {"crates-io", "pypi", "npm", "npm-placeholder"}


@pytest.fixture(scope="module")
def jobs() -> dict:
    """The real workflow's `jobs:` mapping, parsed once for the module."""
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["jobs"]


def needs_of(job: dict) -> set[str]:
    """`needs:` accepts a bare string or a list; normalize to a set."""
    needs = job.get("needs") or []
    return {needs} if isinstance(needs, str) else set(needs)


def upstream_of(name: str, jobs: dict) -> set[str]:
    """Every job that must finish before `name` starts, transitively."""
    seen: set[str] = set()
    stack = list(needs_of(jobs[name]))
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        stack.extend(needs_of(jobs[current]))
    return seen


def publishers(jobs: dict) -> set[str]:
    """The jobs that upload, identified by the environment they enter.

    Derived rather than listed, so a publisher added later is covered without
    this module being touched — and one that loses `environment:` is caught by
    `test_the_publishing_jobs_are_the_ones_we_think` rather than quietly
    dropping out of every check here.
    """
    return {name for name, job in jobs.items() if job.get("environment")}


def waves(jobs: dict) -> dict[str, int]:
    """How many rounds of waiting precede each job.

    A job's wave is one past the latest of its dependencies, so two jobs share
    a wave only if the same round of completions is what unblocks them — which
    is the condition for GitHub raising one approval prompt rather than two.
    """
    depth: dict[str, int] = {}

    def resolve(name: str) -> int:
        """Memoized depth; a job with no dependencies starts at wave 0."""
        if name not in depth:
            depth[name] = 1 + max((resolve(n) for n in needs_of(jobs[name])), default=-1)
        return depth[name]

    return {name: resolve(name) for name in jobs}


def reversible_jobs(jobs: dict) -> set[str]:
    """Everything that neither publishes nor waits on something that does.

    "Waits on" has to be transitive. A job added downstream of
    `github-release` — an announcement, a docs deploy — reaches a publisher
    only through it, and reading that as reversible would have this module
    demand the job run *before* every publisher, which it cannot: the test
    would fail on a correct graph, and a test that cries wolf gets deleted.
    """
    publishing = publishers(jobs)
    downstream = {name for name in jobs if publishing & upstream_of(name, jobs)}
    return set(jobs) - publishing - downstream - {GATE}


# ------------------------------------------------------------------ the graph


def test_every_needs_target_exists(jobs):
    """A typo'd `needs:` is not a YAML error and not an actionlint error in
    every form — it is a job that silently never runs."""
    for name, job in jobs.items():
        missing = needs_of(job) - jobs.keys()
        assert not missing, f"{name} needs undefined job(s): {sorted(missing)}"


def test_the_gate_exists_and_publishes_nothing(jobs):
    """`ready` has to be inert. A gate that itself entered the environment
    would be a fifth pending job — its own approval, before the four it is
    supposed to be converging."""
    assert GATE in jobs, f"the {GATE} job is what makes a release stop exactly once"
    assert "environment" not in jobs[GATE], f"{GATE} must not enter an environment"

    permissions = jobs[GATE].get("permissions")
    assert not permissions, (
        f"{GATE} decides when publishing may start; it should hold no token to do it with"
    )


def test_the_publishing_jobs_are_the_ones_we_think(jobs):
    """Derived from `environment:`, so this catches both directions: a new
    publisher nobody added to this module, and an existing one that lost the
    environment claim every registry checks."""
    assert publishers(jobs) == EXPECTED_PUBLISHERS


def test_every_publisher_uses_the_release_environment(jobs):
    """The trusted publishers on all ten packages are bound to this exact
    name. A typo authenticates nowhere, and only at release time."""
    for name in publishers(jobs):
        assert jobs[name]["environment"] == ENVIRONMENT


# ------------------------------------------------------------- one approval


def test_every_publisher_waits_on_the_gate(jobs):
    """The necessary half of the single-approval property: a publisher that
    does not wait on the gate is unblocked by something else, and asks for its
    own approval when it gets there."""
    for name in publishers(jobs):
        assert GATE in needs_of(jobs[name]), (
            f"{name} enters the {ENVIRONMENT} environment without waiting on {GATE}, "
            f"so it becomes pending in its own wave and asks for its own approval"
        )


def test_no_publisher_waits_on_anything_the_gate_does_not(jobs):
    """The single-approval property itself.

    A publisher may name a job `ready` already waits on — `meta`, for its
    outputs — because that cannot delay it past `ready`. Anything else can, and
    a publisher that becomes unblocked at a different moment than its siblings
    is a second prompt.
    """
    allowed = upstream_of(GATE, jobs) | {GATE}
    for name in publishers(jobs):
        extra = needs_of(jobs[name]) - allowed
        assert not extra, (
            f"{name} waits on {sorted(extra)}, which {GATE} does not — it will go pending "
            f"in a separate wave and split the release into two approvals"
        )


def test_the_publishers_all_unblock_in_the_same_wave(jobs):
    """The property stated the way a maintainer experiences it, and the one
    that fails loudest when the graph drifts back.

    Before the gate existed, `crates-io`, `npm`, and `npm-placeholder` sat one
    wave behind `verify` while `pypi` sat behind the wheel matrix — two waves,
    so two prompts, the second arriving twenty minutes after the first had
    already authorized the release.
    """
    wave = waves(jobs)
    observed = {name: wave[name] for name in publishers(jobs)}
    assert len(set(observed.values())) == 1, (
        f"the publishing jobs unblock in different waves ({observed}), so GitHub will ask "
        f"for an approval once per wave"
    )


# ------------------------------------------------- nothing publishes early


def test_nothing_publishes_before_every_reversible_job_has_passed(jobs):
    """The half-release guard, asserted against each publisher rather than
    against the gate — because it is the publisher's own upstream that decides
    what has been proven when it starts uploading.

    `crates-io: needs [meta, verify]` satisfied a gate-only version of this
    check and still let a failed wheel build strand seven crates on crates.io
    with PyPI empty, at a version neither registry will ever reissue.
    """
    reversible = reversible_jobs(jobs)
    for name in publishers(jobs):
        missing = reversible - upstream_of(name, jobs)
        assert not missing, (
            f"{name} can start before {sorted(missing)} has passed, so that job failing "
            f"would leave a release half-uploaded and unrepairable"
        )


def test_the_gate_waits_for_the_artifacts_the_publishers_upload(jobs):
    """Named explicitly, because the general rule above would still pass if
    these were reclassified. `pypi` uploads what `build-python` and
    `build-sdist` produced; `github-release` needs `changelog`; `verify`
    packages every crate before crates.io ever sees one."""
    for job in ("meta", "verify", "changelog", "build-python", "build-sdist"):
        assert job in upstream_of(GATE, jobs), f"{GATE} must wait for {job}"


def test_the_github_release_comes_after_every_registry(jobs):
    """A GitHub release is the one artifact here that *can* be fixed, so it
    must not be the thing that exists while the packages do not."""
    assert publishers(jobs) <= upstream_of("github-release", jobs)


# ------------------------------------------------------ what the reviewer sees


def test_the_gate_states_what_is_about_to_be_published(jobs):
    """GitHub's approval prompt carries nothing but the environment name, so
    the run has to say what the tag resolved to. Approving on the tag name
    alone would authorize a version and a commit nobody read."""
    steps = jobs[GATE]["steps"]
    summary = "\n".join(step.get("run", "") for step in steps)
    assert "GITHUB_STEP_SUMMARY" in summary

    declared = {key for step in steps for key in (step.get("env") or {})}
    assert {"VERSION", "SHA", "TAG"} <= declared, (
        "the reviewer approves a resolved version and commit, not the tag that was typed"
    )
