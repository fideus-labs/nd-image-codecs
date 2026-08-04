---
title: Publishing
description: How a release reaches crates.io, PyPI, and npm — push a version tag and the release workflow does the rest, with the manual fallback for when it cannot.
---

# Publishing

Releases are published by CI. Push a `vX.Y.Z` tag to `main` and
[`.github/workflows/release.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.github/workflows/release.yml)
stamps that version into every manifest, builds the seven crates, the Python
wheels, and the two npm packages, uploads them, and opens a GitHub release
carrying the changelog. No registry token exists in this repository's secrets —
see [Trusted Publishing](./trusted-publishing.md) for how that authenticates.
Publishing by hand is still possible and documented below, but it is the
fallback for when CI cannot run, not the route a release normally takes.

**The tag decides the version.** Every job runs
[`scripts/release/set-version.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/release/set-version.py)
with the version parsed out of the tag before it builds anything, so `v0.2.0`
publishes 0.2.0 whether or not the tagged commit's manifests say so. What is
committed on `main` is a convenience; the tag is the source of truth.

## Cutting a release

```bash
# 1. From a clean checkout of main, on a branch.
git switch main && git pull
git switch -c release-0.2.0

# 2. Write the version everywhere and add the changelog entry.
scripts/release/prepare-release.sh 0.2.0

# 3. Review the commit, then open a pull request and merge it.
git push -u origin release-0.2.0
gh pr create --fill

# 4. Once merged and CI on main is green, tag the merge commit.
git switch main && git pull
git tag -a v0.2.0 -m "Release 0.2.0"
git push origin v0.2.0

# 5. Watch it publish.
gh run watch --workflow=release.yml
```

Step 2 is optional in the sense that the release still publishes the right
version without it — but skipping it leaves `main` claiming the previous
version, and the release run says so in its job summary. Run it.

Step 4 is the point of no return. crates.io and PyPI both refuse to reuse a
version number, ever, and npm allows an unpublish only within 72 hours and only
if nothing depends on the package.

### What the workflow does

| Job | What it does |
| --- | --- |
| `meta` | Parses the tag, and stops the release here if it is not `v<SemVer>`, if the commit is not an ancestor of `main`, or if CI never went green for it |
| `verify` | Stamps the version, then packages all seven crates and builds each from its own tarball — `cargo publish --workspace --dry-run` |
| `changelog` | `cz changelog <tag>` → the release notes, uploaded as an artifact |
| `crates-io` | Publishes the workspace, skipping crates already at this version |
| `build-python` | Eight wheels: linux/musl/macOS/Windows × x86_64/aarch64 |
| `build-sdist` | The source distribution, plus `twine check` |
| `pypi` | Uploads every wheel and the sdist, with PEP 740 attestations |
| `npm` | Builds the WASM core and TypeScript, tests it, publishes `@fideus-labs/nd-image-codecs` |
| `npm-placeholder` | Publishes the unscoped `nd-image-codecs` name holder |
| `github-release` | Attests the Python distributions, then creates the release with the changelog and attaches them |

`meta` is three gates rather than a step, and it is worth knowing what each
refuses:

- **A tag that is not a release.** The `v*` trigger also matches
  `v2-experiment` and a typo'd `v0.2`.
  [`scripts/release/parse-tag.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/release/parse-tag.py)
  rejects anything that is not `v<MAJOR>.<MINOR>.<PATCH>` with an optional
  `-prerelease`. Build metadata (`+build`) is rejected too: cargo accepts it,
  PyPI has no representation for it, and npm drops it, so it could never mean
  the same version on all three registries.
- **A commit that is not on `main`.** A tag is not a review. Without this,
  anyone able to push a tag could publish an arbitrary commit under this
  project's name.
- **A commit whose CI never passed.** CI does not run on tag pushes, so the
  workflow looks up the `CI` run for the tagged SHA and requires a success. If
  you tag before CI on `main` finishes, this fails; wait and re-run.

## Where the version lives

One published version number is written out **23 times across seven files**.
A partial bump is silent and expensive: `cargo publish` succeeds against a stale
`[workspace.dependencies]` pin, and the resulting crates are unresolvable on
crates.io forever.

| File | Fields |
| --- | --- |
| `Cargo.toml` | `[workspace.package] version`, and the `version` on all 6 internal path deps in `[workspace.dependencies]` |
| `Cargo.lock` | the `[[package]]` entry for each of the 10 workspace members |
| `bindings/python/nd-image-codecs/pyproject.toml` | `[project] version` |
| `bindings/python/nd-image-codecs/python/nd_image_codecs/__init__.py` | the `__version__` import fallback |
| `bindings/typescript/package.json` | `version` |
| `bindings/typescript/package-lock.json` | `version`, twice — top level and `packages[""]` |
| `bindings/javascript/package.json` | `version` |

Two scripts own this, and they are deliberately independent:

- **`scripts/release/set-version.py`** writes all 23. It edits textually rather
  than round-tripping through a TOML or JSON writer, so the comments and
  formatting in these files survive; it needs nothing but `python3`, no cargo
  and no npm.
- **`scripts/ci/check-package-versions.py`** reads all 23 back and asserts they
  agree. The writer runs it before exiting, CI runs it on every pull request,
  and the release workflow runs it after every stamp.

The `package-versions` CI job holds both to account. It **self-tests the
writer** against the real tree — stamp a sentinel version, confirm the reader
sees it, restore, confirm again — and runs
[`scripts/tests/test_release_scripts.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/tests/test_release_scripts.py),
which exercises all three release scripts against a synthetic repository in a
temporary directory: the tag grammar, the third-party pins in both lockfiles
that must not move, the npm formatting that must survive a rewrite, and the
crates.io exclude list. So a file that moves breaks a pull request rather than a
release.

> The internal path deps carry both `path` and `version`. cargo strips `path`
> when packaging and publishes the `version` requirement, so a stale
> `version = "0.1.0"` there makes every downstream crate unresolvable. They bump
> together.
>
> The codec configuration `version` fields (`nd_lift`'s `"0.1"` and friends) are
> a different number entirely — they version the on-disk codec format, not the
> package — and nothing here touches them.

`ndic-bench-core` is deliberately **not** in `[workspace.dependencies]`. It is
`publish = false`, and packaging rewrites every `version`-carrying path dep into
a registry dep — so any published crate naming it fails `cargo publish`, even
behind an off-by-default feature. `ndic-bench-cli` depends on it by bare path.
Do not add it back.

## The changelog

[`CHANGELOG.md`](https://github.com/fideus-labs/nd-image-codecs/blob/main/CHANGELOG.md)
is generated by [commitizen](https://commitizen-tools.github.io/commitizen/)
from the [Conventional Commits](./commits.md) in the range between two tags.
`prepare-release.sh` writes the new section; the release workflow regenerates the
same section for the tag and uses it as the GitHub release body. Both read
[`.cz.toml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.cz.toml),
so the two cannot disagree.

```bash
# Preview the section the next release would carry.
uvx --from commitizen cz changelog --unreleased-version=v0.2.0 --dry-run

# Regenerate the whole file (rebuilds every section from the tags).
uvx --from commitizen cz changelog
```

Sections are emoji-titled and ordered users-first: 💥 Breaking Changes,
✨ Features, 🐛 Bug Fixes, ⚡ Performance, ♻️ Refactoring, 📚 Documentation,
📊 Benchmarks, 🧪 Tests, 📦 Build, 🤖 Continuous Integration, 🎨 Style,
🧹 Chores, ⏪ Reverts.

The configuration widens commitizen's built-in rule, which parses only
`feat`/`fix`/`refactor`/`perf` and would silently drop every `docs:`, `test:`,
`ci:`, and `bench:` commit — 30 of this repository's first 85. `release:`
commits are the one type deliberately excluded: they carry version bumps, which
the release heading already states.

A commit that does not parse as a Conventional Commit does not appear anywhere.
`cz check` validates a message against the same pattern.

### Bootstrapping

`cz changelog --incremental` finds where to resume by matching the newest
version in `CHANGELOG.md` to a **git tag**. Without that tag it prints
`No tag found to do an incremental changelog` and exits 0 — a silent no-op.

0.1.0 was published from a workstation and never tagged, so the tag has to be
created once, pointing at the commit `main` held when 0.1.0 shipped:

```bash
git tag -a v0.1.0 b63e19d -m "Release 0.1.0"
git push origin v0.1.0
```

Pushing it is inert: that commit predates `release.yml`, and GitHub reads
workflow definitions from the pushed ref, so no run is triggered. It exists only
to anchor the changelog.

## When a release fails partway

This is the normal failure mode, not the exceptional one — ten packages across
three registries, none of it reversible. Every publishing step is therefore
idempotent, and **re-running the workflow is the fix**:

**Actions → Release → Run workflow**, and give it the same tag.

| Registry | How the re-run skips what already landed |
| --- | --- |
| crates.io | `publish-crates.py` queries each crate and passes `cargo publish --workspace --exclude` for the ones already at this version. Cargo itself has no skip: `verify_unpublished` bails on the first one, which would otherwise abort the whole workspace |
| PyPI | `skip-existing: true` on the upload |
| npm | Each job checks the registry for the version first |
| GitHub release | Updated in place if it already exists |

If the failure was in the code rather than the pipeline, you cannot fix it under
the same version — publish the fix as the next patch release. A bad version can
be *hidden* (`cargo yank`, `npm deprecate`, PyPI's "yank release") but never
replaced.

## Prereleases

`v1.0.0-rc.1` is a valid release tag and takes the same path, with two
differences the workflow derives from the tag:

- npm publishes under `--tag next`, so `npm install` keeps resolving to the last
  stable release.
- The GitHub release is marked as a prerelease.

crates.io and PyPI both treat a prerelease version as opt-in on their own —
cargo will not resolve a prerelease without an explicit requirement, and `pip`
needs `--pre`. Commitizen folds a prerelease's changelog entries into the
following stable release rather than stranding them.

## Package inventory

| Registry | Package | Source | Publishes |
| --- | --- | --- | --- |
| crates.io | `ndic-core` | [`crates/ndic-core`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-core) | library |
| crates.io | `ndic-htj2k` | [`crates/ndic-htj2k`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-htj2k) | library |
| crates.io | `ndic-lift` | [`crates/ndic-lift`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-lift) | library |
| crates.io | `ndic-zfp` | [`crates/ndic-zfp`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-zfp) | library |
| crates.io | `ndic-codestream` | [`crates/ndic-codestream`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-codestream) | library |
| crates.io | `ndic-zarr` | [`crates/ndic-zarr`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-zarr) | library |
| crates.io | `ndic-cli` | [`crates/ndic-cli`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-cli) | `ndic` binary |
| PyPI | `nd-image-codecs` | [`bindings/python/nd-image-codecs`](https://github.com/fideus-labs/nd-image-codecs/tree/main/bindings/python/nd-image-codecs) | sdist + abi3 wheels |
| npm | `@fideus-labs/nd-image-codecs` | [`bindings/typescript`](https://github.com/fideus-labs/nd-image-codecs/tree/main/bindings/typescript) | ESM + `.d.ts` + WASM |
| npm | `nd-image-codecs` | [`bindings/javascript`](https://github.com/fideus-labs/nd-image-codecs/tree/main/bindings/javascript) | name placeholder, README only |

Three workspace members carry `publish = false` and are skipped automatically:
`ndic-py` (the PyO3 shim — it ships to PyPI inside the wheel, not to crates.io)
and `ndic-bench-core` / `ndic-bench-cli` (the internal benchmark harness).

0.1.0 is live on every one of these except the unscoped npm placeholder, which
is still at 0.0.1. The 0.0.x series were name reservations.

To see what is actually published before cutting a release:

```bash
# The status code is what distinguishes "this name is free" from "the query did
# not work" — a crates.io error body is valid JSON, so parsing alone cannot. 404
# is the free name and prints `none`; anything else reports QUERY FAILED and
# exits non-zero rather than being read as unpublished. `publish-crates.py`
# draws the same line, for the same reason.
versions() { python3 -c '
import sys, json
*body, status = sys.stdin.read().rsplit("\n", 1)
if status == "404":
    print("none"); raise SystemExit(0)
try:
    d = json.loads("\n".join(body))
except json.JSONDecodeError:
    d = {}
if status != "200" or "versions" not in d:
    print("QUERY FAILED"); raise SystemExit(1)
print(",".join(v["num"] for v in d["versions"]) or "none")'; }

for c in ndic-core ndic-htj2k ndic-lift ndic-zfp ndic-codestream ndic-zarr ndic-cli; do
  printf '%-16s ' "$c"
  curl -sS -w '\n%{http_code}' \
    -H "User-Agent: nd-image-codecs (https://github.com/fideus-labs/nd-image-codecs)" \
    "https://crates.io/api/v1/crates/$c" | versions
done
curl -sS https://pypi.org/pypi/nd-image-codecs/json \
  | python3 -c "import sys,json;print(sorted(json.load(sys.stdin)['releases']))"
npm view @fideus-labs/nd-image-codecs versions
npm view nd-image-codecs versions
```

> crates.io returns `403` to a request without a `User-Agent`, so omitting the
> header above reports QUERY FAILED for every crate. That is the header missing,
> not the names being taken.

## Wheel coverage

`abi3-py311` means **one wheel per platform** covers Python 3.11 and every later
version, but each platform still needs its own build. The matrix covers
linux-gnu, linux-musl, macOS, and Windows on both x86_64 and aarch64 — eight
wheels — plus the sdist, so anything else can build from source with a Rust
toolchain.

> `[tool.maturin] features` must keep `pyo3/extension-module`. Without it the
> cdylib links `libpython` and maturin refuses to tag the wheel manylinux. It is
> enabled through maturin rather than in `ndic-py`'s `Cargo.toml` on purpose, so
> that plain `cargo build/test --workspace` keeps linking normally on macOS.

## Publishing by hand

The workflow is the supported path. This is what to do when it is unavailable —
GitHub Actions is down, or the trusted publisher configuration is broken and the
release cannot wait. It needs a crates.io token, a PyPI token, and an npm login,
none of which this project keeps.

```bash
# Stamp the version, prove it landed, and commit it. Publishing from an
# uncommitted stamp would leave the tag pointing at the previous version's
# manifests — and cargo embeds the git revision in the crate it uploads.
scripts/release/prepare-release.sh 0.2.0

# crates.io — dependency order, index waits, and already-published crates
# handled. Set CARGO_REGISTRY_TOKEN or run `cargo login` first.
python3 scripts/release/publish-crates.py 0.2.0 --dry-run
python3 scripts/release/publish-crates.py 0.2.0

# PyPI
SP=$(mktemp -d)
maturin sdist -m bindings/python/nd-image-codecs/Cargo.toml -o "$SP"
maturin build --release -m bindings/python/nd-image-codecs/Cargo.toml -o "$SP"
twine check "$SP"/*
twine upload --repository testpypi "$SP"/*   # rehearse
twine upload "$SP"/*                          # username: __token__

# npm — the WASM core must be built before `npm run build`, and nothing in
# `tsc` notices if it was not. Check the file list.
cd bindings/typescript
npm ci && npm run build:wasm && npm run build && npm test
npm pack --dry-run
npm publish --access public

cd ../javascript && npm publish
cd ../..
```

The crates publish in dependency order:

```text
ndic-core
  ├─ ndic-htj2k ─┐
  ├─ ndic-lift ──┼─ ndic-codestream ─┐
  └─ ndic-zfp ───┴───────────────────┴─ ndic-zarr ─ ndic-cli
```

`cargo publish -p <crate> --dry-run` for a *downstream* crate fails until its
dependencies are on crates.io — the verification build resolves them from the
registry, not from the workspace. The `--workspace` dry run is the one that
proves the whole set.

Then tag the commit that was published — the `release: 0.2.0` commit
`prepare-release.sh` made — and create the GitHub release from it, so the
repository still records what shipped:

```bash
# From the repository root, with $SP still holding the Python distributions.
git tag -a v0.2.0 -m "Release 0.2.0"
git push origin v0.2.0
uvx --from commitizen cz changelog v0.2.0 --dry-run > /tmp/notes.md
gh release create v0.2.0 --title 0.2.0 --notes-file /tmp/notes.md --verify-tag "$SP"/*
```

That tag push will start the release workflow, and unless the `release: 0.2.0`
commit has already been merged to `main` with a green CI run, it will stop at
the `meta` gate — the commit is not an ancestor of `main`, and CI never ran for
its SHA. That failure is safe: `meta` runs before anything is built or
uploaded. Merge the commit and re-run the workflow if you want a green run on
the record; nothing gets published twice either way, because every upload step
finds its version already on the registry and skips.

## Ownership

Every crate should have a second owner so releases are not tied to one account.
A GitHub team works if `fideus-labs` has one (create it first — crates.io
requires the team to exist and you to be a member):

```bash
for c in ndic-core ndic-htj2k ndic-lift ndic-zfp ndic-codestream ndic-zarr ndic-cli; do
  cargo owner --add github:fideus-labs:publishers "$c"
done
```

## Notes

- **Nothing here is reversible.** crates.io and PyPI both forbid reusing a
  version number; npm allows unpublish within 72 hours, and only if nothing
  depends on the package. `cargo yank` hides a version from new resolution but
  does not delete it, and the number can never be reused.
- **Registry policy.** crates.io and npm both reserve the right to reclaim names
  held purely for squatting. Each published package therefore ships a real
  README, a license, a repository link, and — for Rust and Python — working
  `codec_series` code. Keep publishing as releases land so the names stay
  clearly in use.
- **The release workflow's filename is load-bearing.** crates.io and npm bind
  their trusted publishers to `release.yml` specifically, and PyPI binds to it
  too; renaming it breaks authentication on all ten packages, and only at
  release time.
