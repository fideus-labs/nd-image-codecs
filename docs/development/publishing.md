---
title: Publishing
description: How to publish nd-image-codecs to crates.io, PyPI, and npm by hand, including the 0.0.1 name-reservation release.
---

# Publishing

How to publish nd-image-codecs to crates.io, PyPI, and npm **by hand**. There is
no release automation yet; every step here is run from a workstation.

The current target is **0.0.1** — a name-reservation release. The `codec_series`
builder is real and works in all three languages; the codec encode/decode paths
are scaffolds (see the [roadmap](./roadmap/index.md)). Publishing 0.0.1 claims
the names on every registry and gives each one a real README, repository link,
and license so the packages read as an early-stage project rather than an empty
squat.

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

All ten names above were confirmed unregistered on 2026-08-01. Check again
immediately before you publish:

```sh
curl -s -H "User-Agent: nd-image-codecs (matt@fideus.io)" \
  -o /dev/null -w '%{http_code}\n' https://crates.io/api/v1/crates/ndic-core   # 404 = free
curl -s -o /dev/null -w '%{http_code}\n' https://pypi.org/pypi/nd-image-codecs/json
curl -s -o /dev/null -w '%{http_code}\n' https://registry.npmjs.org/nd-image-codecs
```

> crates.io returns `403` to requests without a `User-Agent`; that is not a name
> collision. Only `200` means taken.

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

`wasm-pack` is **not** required for 0.0.1 — `bindings/typescript/src/index.ts` is
pure TypeScript today, so `tsc` alone produces the published artifact. It becomes
required once the WASM codec cores land (Phases 2–5).

## 0. Pre-flight

Run from the repository root.

```sh
# 1. The tree must be clean and pushed — cargo embeds the git revision.
git status --porcelain          # must be empty
git switch main && git pull

# 2. Version must read 0.0.1 in every manifest.
rg -n '"0\.0\.1"|version = "0\.0\.1"' Cargo.toml \
  bindings/python/nd-image-codecs/pyproject.toml \
  bindings/typescript/package.json \
  bindings/javascript/package.json

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
| `Cargo.toml` | `[workspace.dependencies]` — the `version = "…"` on all 7 internal path deps |
| `bindings/python/nd-image-codecs/pyproject.toml` | `[project] version` |
| `bindings/python/nd-image-codecs/python/nd_image_codecs/__init__.py` | `__version__` import fallback |
| `bindings/typescript/package.json` | `version` |
| `bindings/javascript/package.json` | `version` |

The internal path deps carry both `path` and `version`. cargo strips `path` when
packaging and publishes the `version` requirement, so a stale `version = "0.0.0"`
there makes every downstream crate unresolvable on crates.io. Bump them together.

## 1. Rust → crates.io

The seven crates form a dependency chain; crates.io needs each dependency
published (and indexed) before its dependents. `cargo publish --workspace`
computes the order, skips `publish = false` members, and waits for the index
between uploads — use it rather than publishing by hand.

```sh
# Dry run: packages every crate and verifies each builds from its own tarball.
cargo publish --workspace --dry-run
```

Expected: seven `Packaging …` lines, seven `Uploading …` lines, each followed by
`warning: aborting upload due to dry run`. Then:

```sh
cargo login                 # once; or export CARGO_REGISTRY_TOKEN
cargo publish --workspace
```

The resolved order is:

```
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

```sh
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

```sh
for c in ndic-core ndic-htj2k ndic-lift ndic-zfp ndic-codestream ndic-zarr ndic-cli; do
  cargo owner --add github:fideus-labs:publishers "$c"   # or: cargo owner --add <username> "$c"
done
```

Verify:

```sh
cargo search ndic-
cargo install ndic-cli --version 0.0.1 && ndic --help
```

> Published versions are **permanent**. `cargo yank --version 0.0.1 <crate>`
> hides a version from new resolution but does not delete it, and the version
> number can never be reused.

## 2. Python → PyPI

The Python distribution is built by maturin from
`bindings/python/nd-image-codecs/pyproject.toml`. It is a mixed Python/Rust
project: pure-Python sources under `python/`, plus the `ndic-py` cdylib built as
a stable-ABI (`abi3-py311`) extension module.

```sh
SP=$(mktemp -d)

# sdist — the important artifact for a name reservation: anyone can build from it.
maturin sdist -m bindings/python/nd-image-codecs/Cargo.toml -o "$SP"

# wheel for the current platform.
maturin build --release -m bindings/python/nd-image-codecs/Cargo.toml -o "$SP"

twine check "$SP"/*
```

Smoke-test the wheel before uploading:

```sh
uv venv "$SP/venv" --python 3.12
uv pip install --python "$SP/venv/bin/python" "$SP"/*.whl
"$SP/venv/bin/python" -c "import nd_image_codecs as m; print(m.__version__)"   # 0.0.1
```

Upload — TestPyPI first, then the real index:

```sh
twine upload --repository testpypi "$SP"/*
twine upload "$SP"/*     # username: __token__ , password: the pypi-… token
```

`maturin publish -m bindings/python/nd-image-codecs/Cargo.toml -u __token__`
builds and uploads in one step; the split above is preferred because it lets you
run `twine check` and the import smoke-test against exactly the files you ship.

### Wheel coverage

`abi3-py311` means **one wheel per platform** covers Python 3.11+ — but each
platform still needs its own build. For 0.0.1, shipping the sdist plus whatever
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

```sh
cd bindings/typescript
npm install
npm run build            # tsc -p tsconfig.json → dist/
npm test                 # vitest — see note below
npm pack --dry-run       # inspect the file list
```

> `npm test` currently exits `1` with `No test files found`: no `*.test.ts`
> exists yet. That is expected for 0.0.1 and is not a publish blocker — the
> build is what matters. Once the first test lands the command goes green.

Expected tarball: `README.md`, `package.json`, `dist/index.{js,d.ts,js.map,d.ts.map}`,
`src/index.ts`.

```sh
npm login
npm publish --access public
```

`--access public` is required on the **first** publish of a scoped package;
without it npm defaults the package to restricted and the publish fails on a
free account. Subsequent publishes inherit the setting. If the account has 2FA
on publish, append `--otp=123456`.

Once the WASM cores land, `npm run build:wasm` (which needs `wasm-pack`) must run
before `npm run build`, and the wasm artifacts have to be added to the `files`
list in `package.json`.

## 4. JavaScript placeholder → npm

[`bindings/javascript`](https://github.com/fideus-labs/nd-image-codecs/tree/main/bindings/javascript) reserves the **unscoped**
`nd-image-codecs` name on npm. It contains a README pointing at the scoped
package and nothing else — no code, no dependency on the scoped package, so it
never needs to be kept in version lockstep beyond whatever you choose to publish.

```sh
cd bindings/javascript
npm pack --dry-run       # README.md + package.json only
npm publish              # unscoped packages are public by default
```

If the unscoped name is later wanted as the real package, publish the TypeScript
build under it and deprecate the scope — or the reverse:

```sh
npm deprecate nd-image-codecs@0.0.1 "Use @fideus-labs/nd-image-codecs instead"
```

## 5. Verify, tag, record

```sh
# crates.io
curl -s -H "User-Agent: nd-image-codecs (matt@fideus.io)" \
  https://crates.io/api/v1/crates/ndic-zarr | head -c 200

# PyPI
pip download --no-deps --no-binary :all: nd-image-codecs==0.0.1 -d /tmp/verify

# npm
npm view @fideus-labs/nd-image-codecs@0.0.1
npm view nd-image-codecs@0.0.1
```

Then tag the commit that was published:

```sh
git tag -a v0.0.1 -m "Release 0.0.1 — name reservation"
git push origin v0.0.1
```

Create a GitHub release against the tag noting that 0.0.1 reserves the names and
ships only the `codec_series` builder.

## Release checklist

- [ ] `main` clean, pulled, and CI green
- [ ] Version reads `0.0.1` in all six locations (table above)
- [ ] `cargo publish --workspace --dry-run` clean
- [ ] `cargo publish --workspace`
- [ ] `cargo owner --add` on all seven crates
- [ ] `maturin sdist` + `maturin build --release`, `twine check`, import smoke-test
- [ ] `twine upload --repository testpypi`, then `twine upload`
- [ ] `bindings/typescript`: build, test, `npm publish --access public`
- [ ] `bindings/javascript`: `npm publish`
- [ ] Installs verified from all three registries
- [ ] `v0.0.1` tagged and pushed; GitHub release created

## Notes

- **Order matters across registries only for npm**, and only if the placeholder
  is ever made to depend on the scoped package. crates.io, PyPI, and npm are
  otherwise independent.
- **Nothing here is reversible.** crates.io and PyPI both forbid reusing a
  version number; npm allows unpublish within 72 hours, and only if nothing
  depends on the package. Rehearse with the dry runs and TestPyPI.
- **Registry policy.** crates.io and npm both reserve the right to reclaim names
  held purely for squatting. Each 0.0.1 package therefore ships a real README, a
  license, a repository link, and — for Rust and Python — working
  `codec_series` code. Keep publishing as the roadmap phases land so the names
  stay clearly in use.
