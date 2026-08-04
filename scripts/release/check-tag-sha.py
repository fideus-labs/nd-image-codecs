#!/usr/bin/env python3
"""Refuse a release whose tag has been moved since an earlier run of it.

`meta` already proves the *current* tag commit is on `main` and went green in
CI, and every downstream job checks out that one resolved SHA rather than
re-reading the tag — so within a single run the tag cannot change under the
release. This closes the gap *between* runs.

Re-running a release is the supported way to finish one that died partway
(`docs/development/publishing.md`, "When a release fails partway"), and the
skip logic keys on the version, not the source. So if the tag is moved between
the first run and the re-run, the packages that already landed came from one
commit and the ones still to publish come from another — a release that is
internally inconsistent and nothing downstream would notice.

There is no place to persist "the SHA this tag was released from", but there
does not need to be: GitHub already keeps it. Every `push`-event run of the
release workflow records the commit its tag pointed at, and a release always
*starts* with a tag push. Those runs are the record, and this compares against
them.

The whole class is better prevented than detected — a tag ruleset with
"Restrict updates" makes a release tag immutable, and then this check can never
fire. `docs/development/trusted-publishing.md` has the setup. This is the
backstop for a repository that has not configured it, and it necessarily says
nothing about a release whose first run was a `workflow_dispatch`.

Reads the `GET /repos/{repo}/actions/workflows/{id}/runs` response on stdin:

    gh api "repos/$REPO/actions/workflows/release.yml/runs?per_page=100" |
      python3 scripts/release/check-tag-sha.py --tag v0.2.0 --sha "$SHA"

Requires: python3 only. The caller does the authenticated fetch.
"""

from __future__ import annotations

import argparse
import json
import sys


def prior_push_runs(payload: dict | list, tag: str) -> list[dict]:
    """Runs of this workflow started by pushing `tag`.

    Only `push` runs, because only they carry an unambiguous answer: the head
    SHA of a tag-push run is the commit the tag pointed at when it was pushed.
    A `workflow_dispatch` run's head SHA is whichever ref the operator selected
    in the UI, which is the tag only by convention, so including those would
    turn "someone dispatched from main" into a spurious moved-tag failure.
    """
    runs = payload if isinstance(payload, list) else payload.get("workflow_runs", [])
    return [
        run
        for run in runs
        if run.get("event") == "push"
        # The runs API reports a tag push's ref in `head_branch`, short-form.
        # The long form is accepted too rather than assumed against.
        and run.get("head_branch") in (tag, f"refs/tags/{tag}")
        and run.get("head_sha")
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tag", required=True, help="the release tag, e.g. v0.2.0")
    parser.add_argument("--sha", required=True, help="the commit this run resolved the tag to")
    args = parser.parse_args()

    try:
        payload = json.load(sys.stdin)
    except json.JSONDecodeError as error:
        # Fails closed: an unreadable run list is not evidence that the tag has
        # not moved, and the thing being guarded cannot be undone.
        print(f"check-tag-sha.py: could not parse the workflow runs: {error}", file=sys.stderr)
        return 1

    runs = prior_push_runs(payload, args.tag)
    disagree = sorted({run["head_sha"] for run in runs} - {args.sha})

    if disagree:
        print(f"::error::{args.tag} has been moved since it was first released.", file=sys.stderr)
        print(
            f"\nThis run resolved {args.tag} to {args.sha}, but earlier runs of the release\n"
            f"workflow saw it at:\n",
            file=sys.stderr,
        )
        for sha in disagree:
            when = sorted(r.get("created_at", "?") for r in runs if r["head_sha"] == sha)
            print(f"  {sha}  (first seen {when[0]})", file=sys.stderr)
        print(
            "\nRe-running would publish the not-yet-published packages from a different\n"
            "commit than the ones already on the registries. Releases are immutable, so\n"
            "the fix is a new version, not a moved tag: restore the tag to the commit\n"
            "above, or cut the next patch release.\n",
            file=sys.stderr,
        )
        return 1

    if runs:
        print(f"{len(runs)} earlier run(s) of {args.tag} all saw {args.sha}.")
    else:
        print(f"No earlier tag-push run of {args.tag}; nothing to compare against.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
