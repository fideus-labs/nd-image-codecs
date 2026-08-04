---
title: Publishing
description: How to publish nd-image-codecs to crates.io, PyPI, and npm by hand, including the 0.1.0 release that ships the working codecs.
---

# Publishing

How to publish nd-image-codecs to crates.io, PyPI, and npm **by hand**. There is
no release automation yet; every step here is run from a workstation.

The current target is **0.1.0** — the first release with working codecs rather
than name reservations. `nd_lift`, `nd_lift_ht`, and `nd_zfp` encode and decode
for real, the `codec_series` builder works in all three languages, and the
TypeScript package ships a WASM core. The minor bump signals that: 0.0.x was a
placeholder series.

> **The 0.0.x releases were name reservations, and 0.0.2 only landed in part.**
> 0.0.1 (2026-08-01) reached PyPI and both npm packages, plus five crates —
> `ndic-core`, `ndic-htj2k`, `ndic-lift`, `ndic-zfp`, `ndic-codestream`. It
> stopped on `ndic-zarr`, whose `ndic-lift/serde` feature had been added after
> `ndic-lift 0.0.1` was uploaded, and a published version cannot be amended.
> 0.0.2 re-published the whole set in lockstep and completed crates.io — all
> seven crates are there — but PyPI and npm were never bumped past 0.0.1. So
> 0.1.0 is the first 0.1 upload on every registry, and on PyPI/npm it is the
> first upload since 0.0.1.

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
| npm | `@fideus-labs/nd-image-codecs` | [`bindings/typescript`](https://github.com/fideus-labs/nd-image-codecs/tree/main/bindings/typescript) | ESM + `.d.ts` |
| npm | `nd-image-codecs` | [`bindings/javascript`](https://github.com/fideus-labs/nd-image-codecs/tree/main/bindings/javascript) | name placeholder, README only |

**Not published.** Three workspace members carry `publish = false` and are
skipped automatically by `cargo publish --workspace`:

- `ndic-py` — the PyO3 shim; it ships to PyPI inside the wheel, not to crates.io.
- `ndic-bench-core`, `ndic-bench-cli` — internal benchmark harness.

Eight of the ten names are now ours; only `ndic-zarr` and `ndic-cli` remain
unregistered on crates.io. Confirm which versions are live before you publish —
a version already on a registry cannot be re-uploaded:

```bash
# `-sS` keeps the progress meter off but still prints transfer errors, and an
# unparseable body reports QUERY FAILED rather than being read as "unpublished".
# A 404 for a name that is genuinely free returns valid JSON and prints `none`.
versions() { python3 -c '
import sys, json
try:
    d = json.loads(sys.stdin.read())
except json.JSONDecodeError:
    print("QUERY FAILED"); raise SystemExit(1)
print(",".join(v["num"] for v in d.get("versions", [])) or "none")'; }

for c in ndic-core ndic-htj2k ndic-lift ndic-zfp ndic-codestream ndic-zarr ndic-cli; do
  printf '%-16s ' "$c"
  curl -sS -H "User-Agent: nd-image-codecs (matt@fideus.io)" \
    "https://crates.io/api/v1/crates/$c" | versions
done
curl -sS https://pypi.org/pypi/nd-image-codecs/json \
  | python3 -c "import sys,json;print(sorted(json.load(sys.stdin)['releases']))"
npm view @fideus-labs/nd-image-codecs versions
npm view nd-image-codecs versions
```

> crates.io returns `403` to requests without a `User-Agent`; that is not a name
> collision.

## Prerequisites

| Need | Get it |
| --- | --- |
| crates.io account + token | <https://crates.io/settings/tokens>, then `cargo login` |
| PyPI account + API token | <https://pypi.org/manage/account/token/> (scope it to the project after first upload) |
| npm account, member of the `fideus-labs` org | `npm login`; create the org at <https://www.npmjs.com/org/create> if it does not exist yet — the scope must exist before the scoped package can be published |
| Rust 1.91 | pinned by [`rust-toolchain.toml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/rust-toolchain.toml) |
| maturin ≥ 1.7 | `pipx install maturin` or `uv tool install maturin` |
| twine | `pipx install twine` |
| Node 20+ | for `npm publish` |

`wasm-pack` **is** required as of 0.1.0: the TypeScript package loads a WASM core
built from `crates/ndic-zarr`, so `npm run build:wasm` has to run before
`npm run build`. Install it with `cargo install wasm-pack` (CI uses
`taiki-e/install-action`).

## 0. Pre-flight

Run from the repository root.

```bash
# 1. The tree must be clean and pushed — cargo embeds the git revision.
git status --porcelain          # must be empty
git switch main && git pull

# 2. Every version location must agree. This parses each file rather than
#    grepping it — a bare `rg '0\.1\.0'` passes on one match while the rest are
#    stale, and package-lock.json legitimately pins third-party packages at
#    old versions (stackback is 0.0.2), which a grep cannot tell from a stale
#    project field. Also runs in CI (the `package-versions` job).
python3 scripts/ci/check-package-versions.py

# 3. Everything green.
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
python3 scripts/ci/check-series-equality.py
```

### Where the version lives

Bumping a release means editing exactly these, then running `cargo check
--workspace` to refresh `Cargo.lock`:

| File | Field |
| --- | --- |
| `Cargo.toml` | `[workspace.package] version` |
| `Cargo.toml` | `[workspace.dependencies]` — the `version = "…"` on all 6 internal path deps |
| `bindings/python/nd-image-codecs/pyproject.toml` | `[project] version` |
| `bindings/python/nd-image-codecs/python/nd_image_codecs/__init__.py` | `__version__` import fallback |
| `bindings/typescript/package.json` | `version` |
| `bindings/typescript/package-lock.json` | `version`, twice (top level and `packages.""`) |
| `bindings/javascript/package.json` | `version` |

The internal path deps carry both `path` and `version`. cargo strips `path` when
packaging and publishes the `version` requirement, so a stale `version = "0.0.0"`
there makes every downstream crate unresolvable on crates.io. Bump them together.

`scripts/ci/check-package-versions.py` asserts all of the above agree, and CI
runs it on every push — so a partial bump fails the branch rather than the
release. It covers `Cargo.lock` too, which is why the `cargo check` above is
what keeps that file honest.

`ndic-bench-core` is deliberately **not** in `[workspace.dependencies]`. It is
`publish = false`, and packaging rewrites every `version`-carrying path dep into
a registry dep — so any published crate naming it fails `cargo publish`, even
behind an off-by-default feature. `ndic-bench-cli` depends on it by bare path,
and the benchmark workloads live in the driver (`bench/rs/ndic-bench-cli/src/workloads/`)
rather than in the codec crates. Do not add it back.

## 1. Rust → crates.io

The seven crates form a dependency chain; crates.io needs each dependency
published (and indexed) before its dependents. `cargo publish --workspace`
computes the order, skips `publish = false` members, and waits for the index
between uploads — use it rather than publishing by hand.

```bash
# Dry run: packages every crate and verifies each builds from its own tarball.
cargo publish --workspace --dry-run
```

Expected: seven `Packaging …` lines, seven `Uploading …` lines, each followed by
`warning: aborting upload due to dry run`. Then:

```bash
cargo login                 # once; or export CARGO_REGISTRY_TOKEN
cargo publish --workspace
```

The resolved order is:

```text
ndic-core
  ├─ ndic-htj2k ─┐
  ├─ ndic-lift ──┼─ ndic-codestream ─┐
  └─ ndic-zfp ───┴───────────────────┴─ ndic-zarr ─ ndic-cli
```

<details>
<summary>Fallback: one crate at a time</summary>

If `--workspace` fails partway through, publish the remainder individually in
the order above. crates.io serves a crate to the resolver a few seconds after
upload, so pause between them:

```bash
for c in ndic-core ndic-htj2k ndic-lift ndic-zfp ndic-codestream ndic-zarr ndic-cli; do
  cargo publish -p "$c" || break
  sleep 30
done
```

`cargo publish -p <crate> --dry-run` for a *downstream* crate fails until its
dependencies are on crates.io — the dry-run verification build resolves them
from the registry, not from the workspace. That is expected; the
`--workspace` dry run above is the one that proves the whole set.

</details>

### After the first publish

Add a second owner on every crate so releases are not tied to one account. A
GitHub team works if `fideus-labs` has one (create it first — crates.io requires
the team to already exist and you to be a member); otherwise add individual
crates.io usernames.

```bash
for c in ndic-core ndic-htj2k ndic-lift ndic-zfp ndic-codestream ndic-zarr ndic-cli; do
  cargo owner --add github:fideus-labs:publishers "$c"   # or: cargo owner --add <username> "$c"
done
```

Verify:

```bash
cargo search ndic-
cargo install ndic-cli --version 0.1.0 && ndic --help
```

> Published versions are **permanent**. `cargo yank --version 0.1.0 <crate>`
> hides a version from new resolution but does not delete it, and the version
> number can never be reused.

## 2. Python → PyPI

The Python distribution is built by maturin from
`bindings/python/nd-image-codecs/pyproject.toml`. It is a mixed Python/Rust
project: pure-Python sources under `python/`, plus the `ndic-py` cdylib built as
a stable-ABI (`abi3-py311`) extension module.

```bash
SP=$(mktemp -d)

# sdist — the important artifact for a name reservation: anyone can build from it.
maturin sdist -m bindings/python/nd-image-codecs/Cargo.toml -o "$SP"

# wheel for the current platform.
maturin build --release -m bindings/python/nd-image-codecs/Cargo.toml -o "$SP"

twine check "$SP"/*
```

Smoke-test the wheel before uploading:

```bash
uv venv "$SP/venv" --python 3.12
uv pip install --python "$SP/venv/bin/python" "$SP"/*.whl
"$SP/venv/bin/python" -c "import nd_image_codecs as m; print(m.__version__)"   # 0.1.0
```

Upload — TestPyPI first, then the real index:

```bash
twine upload --repository testpypi "$SP"/*
twine upload "$SP"/*     # username: __token__ , password: the pypi-… token
```

`maturin publish -m bindings/python/nd-image-codecs/Cargo.toml -u __token__`
builds and uploads in one step; the split above is preferred because it lets you
run `twine check` and the import smoke-test against exactly the files you ship.

### Wheel coverage

`abi3-py311` means **one wheel per platform** covers Python 3.11+ — but each
platform still needs its own build. For 0.1.0, shipping the sdist plus whatever
wheels you can build locally is enough; users on other platforms fall back to
building from the sdist (which needs a Rust toolchain). A full
linux/macOS/Windows × x86_64/aarch64 matrix is a job for `maturin-action` in CI
when there is real code to ship.

> `[tool.maturin] features` must keep `pyo3/extension-module`. Without it the
> cdylib links `libpython` and maturin refuses to tag the wheel manylinux. It is
> enabled through maturin rather than in `ndic-py`'s `Cargo.toml` on purpose, so
> that plain `cargo build/test --workspace` keeps linking normally on macOS.

> PyPI project names cannot be reused after deletion, and a released version
> number cannot be re-uploaded. Use TestPyPI to rehearse.

## 3. TypeScript → npm

`@fideus-labs/nd-image-codecs` is the real package. It ships compiled ESM plus
type declarations from `dist/`, and the TypeScript sources so the shipped source
maps resolve.

```bash
cd bindings/typescript
npm install
npm run build:wasm       # wasm-pack → src/wasm/ (required before build)
npm run build            # tsc → dist/, then copies src/wasm → dist/wasm
npm test                 # vitest
npm pack --dry-run       # inspect the file list
```

The WASM artifacts ride along in `dist/wasm/`, which the existing
`files: ["dist", "src"]` already ships — no `files` edit needed. Publishing
without running `build:wasm` first produces a tarball whose codecs cannot
initialize, and nothing in `tsc` catches that, so check the file list.

Expected tarball: `README.md`, `package.json`, `dist/index.{js,d.ts,js.map,d.ts.map}`,
`dist/zarrita.{js,d.ts,js.map,d.ts.map}`, `dist/wasm/*`, and the `src/` sources.

```bash
npm login
npm publish --access public
```

`--access public` is required on the **first** publish of a scoped package;
without it npm defaults the package to restricted and the publish fails on a
free account. Subsequent publishes inherit the setting. If the account has 2FA
on publish, append `--otp=123456`.

## 4. JavaScript placeholder → npm

[`bindings/javascript`](https://github.com/fideus-labs/nd-image-codecs/tree/main/bindings/javascript) reserves the **unscoped**
`nd-image-codecs` name on npm. It contains a README pointing at the scoped
package and nothing else — no code, no dependency on the scoped package, so it
never needs to be kept in version lockstep beyond whatever you choose to publish.

```bash
cd bindings/javascript
npm pack --dry-run       # README.md + package.json only
npm publish              # unscoped packages are public by default
```

If the unscoped name is later wanted as the real package, publish the TypeScript
build under it and deprecate the scope — or the reverse:

```bash
npm deprecate nd-image-codecs@0.0.1 "Use @fideus-labs/nd-image-codecs instead"
```

## 5. Verify, tag, record

```bash
# crates.io
curl -s -H "User-Agent: nd-image-codecs (matt@fideus.io)" \
  https://crates.io/api/v1/crates/ndic-zarr | head -c 200

# PyPI
pip download --no-deps --no-binary :all: nd-image-codecs==0.1.0 -d /tmp/verify

# npm
npm view @fideus-labs/nd-image-codecs@0.1.0
npm view nd-image-codecs@0.1.0
```

Then tag the commit that was published:

```bash
git tag -a v0.1.0 -m "Release 0.1.0 — working nd_lift, nd_lift_ht, and nd_zfp codecs"
git push origin v0.1.0
```

Create a GitHub release against the tag noting that 0.1.0 is the first release
with working codecs, and that it is the first PyPI/npm upload since 0.0.1.

## Release checklist

- [ ] `main` clean, pulled, and CI green
- [ ] `python3 scripts/ci/check-package-versions.py` green (all version locations agree)
- [ ] `cargo publish --workspace --dry-run` clean
- [ ] `cargo publish --workspace`
- [ ] `cargo owner --add` on all seven crates
- [ ] `maturin sdist` + `maturin build --release`, `twine check`, import smoke-test
- [ ] `twine upload --repository testpypi`, then `twine upload`
- [ ] `bindings/typescript`: `npm run build:wasm` then `npm run build`; `npm test` green; `npm publish --access public`
- [ ] `bindings/javascript`: `npm publish`
- [ ] Installs verified from all three registries
- [ ] `v0.1.0` tagged and pushed; GitHub release created

## Notes

- **Order matters across registries only for npm**, and only if the placeholder
  is ever made to depend on the scoped package. crates.io, PyPI, and npm are
  otherwise independent.
- **Nothing here is reversible.** crates.io and PyPI both forbid reusing a
  version number; npm allows unpublish within 72 hours, and only if nothing
  depends on the package. Rehearse with the dry runs and TestPyPI.
- **Registry policy.** crates.io and npm both reserve the right to reclaim names
  held purely for squatting. Each published package therefore ships a real README, a
  license, a repository link, and — for Rust and Python — working
  `codec_series` code. Keep publishing as releases land so the names stay
  clearly in use.
