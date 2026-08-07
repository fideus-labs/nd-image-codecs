#!/usr/bin/env bash
# Stage a release commit: version numbers, then the changelog entry for them.
#
# This is the ergonomic half of the release. The authoritative half is the tag
# — `.github/workflows/release.yml` stamps the version out of `vX.Y.Z` into
# every manifest before it builds, so a release published from a tag is correct
# whether or not this script ever ran. What this adds is that `main` afterwards
# says what was released: the version in the manifests, the version the usage
# documentation tells a reader to depend on, and the changelog entry the GitHub
# release will carry.
#
# Run it, open a pull request, merge it, then tag the merge commit. Tagging
# without it works and only costs you a drift warning in the release run.
#
#     scripts/release/prepare-release.sh 0.2.0
#
# Requires: git, python3, and commitizen — via `cz` on PATH, or `uvx`, or
# `pipx`, whichever is present.

set -euo pipefail

# Kept in step with the pin in .github/workflows/release.yml, so the changelog
# written here and the release notes generated there come from one generator.
CZ_VERSION="4.17.0"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
step() { printf '\n\033[1m==>\033[0m %s\n' "$*"; }

[ $# -eq 1 ] || die "usage: scripts/release/prepare-release.sh <version>   (e.g. 0.2.0)"
VERSION="$1"
TAG="v${VERSION}"

# `cz changelog --incremental` finds the boundary by matching the newest
# version in CHANGELOG.md to a *git tag*. Without one it prints "No tag found
# to do an incremental changelog" and exits 0, leaving the file untouched — a
# silent no-op that would otherwise be discovered at release time.
resolve_cz() {
  # A `cz` on PATH is used only if it is the pinned version. The release
  # workflow generates the GitHub release body with CZ_VERSION, and a local
  # commitizen of a different version can section or order the same commits
  # differently — so CHANGELOG.md and the release notes would disagree, which
  # is the one thing sharing .cz.toml between them is meant to prevent.
  if command -v cz >/dev/null 2>&1 && [ "$(cz version 2>/dev/null)" = "$CZ_VERSION" ]; then
    echo "cz"
  elif command -v uvx >/dev/null 2>&1; then
    echo "uvx --from commitizen==${CZ_VERSION} cz"
  elif command -v pipx >/dev/null 2>&1; then
    echo "pipx run --spec commitizen==${CZ_VERSION} cz"
  else
    die "commitizen not found. Install uv (https://docs.astral.sh/uv/) or run: pipx install commitizen==${CZ_VERSION}"
  fi
}
CZ="$(resolve_cz)"

step "Checking the working tree"
[ -z "$(git status --porcelain)" ] || die "the working tree is dirty; commit or stash first"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[ "$BRANCH" != "HEAD" ] || die "detached HEAD; check out a branch first"
git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null &&
  die "${TAG} already exists. A published version cannot be re-released; pick the next one."
echo "on ${BRANCH}, clean"

# The changelog's previous entry needs a tag behind it or the range is wrong.
# --match restricts this to release tags. Without it the newest reachable tag of
# any shape wins — a `v2-experiment`, a vendor tag, anything — and the changelog
# range would start from something that was never released.
PREVIOUS_TAG="$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)"
if [ -z "$PREVIOUS_TAG" ]; then
  cat >&2 <<'EOF'

warning: this repository has no release tags yet.

  The changelog entry will therefore cover the entire history rather than the
  commits since the last release, and `--incremental` will have nothing to
  anchor to on the next release. If earlier versions were published, tag their
  commits first — see docs/development/publishing.md, "Bootstrapping the
  changelog".

EOF
  read -r -p "Continue anyway? [y/N] " reply
  [ "$reply" = "y" ] || [ "$reply" = "Y" ] || die "stopped"
else
  echo "previous release: ${PREVIOUS_TAG}"
  if [ "$(git rev-list --count "${PREVIOUS_TAG}..HEAD")" -eq 0 ]; then
    die "no commits since ${PREVIOUS_TAG} — there is nothing to release"
  fi
fi

# From here on the tree gets rewritten, so a failure has to say so; otherwise
# it looks like the script was a no-op and the manifests quietly carry a
# version that was never released.
trap 'printf "\n\033[31mstopped with the working tree modified.\033[0m Undo it with:\n  git checkout -- .\n" >&2' ERR

# Manifests and lockfiles, and the dependency pins in `docs/usage/*.md` — the
# usage-docs CI job runs those pages against this workspace, so a bump that left
# them behind would fail the release pull request on `docs/usage/rust.md`.
step "Writing ${VERSION} into every manifest and usage page"
python3 scripts/release/set-version.py "$VERSION"

step "Adding the ${TAG} changelog entry"
# `--unreleased-version` labels the not-yet-created tag, which is the whole
# chicken-and-egg of writing a changelog in the commit that precedes the tag.
if [ -n "$PREVIOUS_TAG" ]; then
  $CZ changelog --incremental --unreleased-version="$TAG"
else
  $CZ changelog --unreleased-version="$TAG"
fi

step "Committing"
git add -A
git commit -m "release: ${VERSION}" -m "Version bump across crates.io, PyPI, and npm, plus the ${TAG} changelog entry."

cat <<EOF

$(git show --stat --oneline HEAD | head -20)

Next:

  1. Push and open a pull request:

       git push -u origin ${BRANCH}
       gh pr create --fill

  2. Merge it, and wait for CI on main to go green — the release workflow
     refuses to publish a commit without a successful CI run.

  3. Tag the merge commit and push the tag. That is what publishes:

       git switch main && git pull
       git tag -a ${TAG} -m "Release ${VERSION}"
       git push origin ${TAG}

  4. Watch it: gh run watch --workflow=release.yml

Nothing is reversible once step 3 lands. docs/development/publishing.md has the
runbook, including how to finish a release that failed partway.
EOF
