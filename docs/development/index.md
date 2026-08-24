---
title: Development
description: 'The map of the nd-image-codecs contributor documentation: how to build, test, benchmark, and release the project, and the conventions every change is held to.'
---

# Development

This index is the map of the nd-image-codecs contributor documentation: how to build,
test, benchmark, and release the project, and the conventions every change is held to.
Open the page that matches the task in front of you.

## Working on the project

| Document | What it covers |
| --- | --- |
| [Development Commands](./commands.md) | Everyday commands: build and check, test, lint and format, benchmarks, bindings, CLI smoke tests, release, and building the documentation site |
| [Benchmarking](./benchmarking.md) | Running and adding benchmarks — record layout, named baselines, and the PR regression gate |
| [Test Data](./test-data.md) | Test data and the conformance corpus: what is vendored, what is downloaded, and where it is cached |
| [Publishing](./publishing.md) | Cutting a release: the `vX.Y.Z` tag that publishes to crates.io, PyPI, and npm, where the version lives, the changelog, and what to do when a release fails partway |
| [Trusted Publishing](./trusted-publishing.md) | The one-time registry setup behind that workflow — the `release` environment and ten OpenID Connect publisher registrations, so no API token is stored anywhere |
| [Read the Docs](./read-the-docs.md) | Deploying the documentation site to Read the Docs — the build recipe in the repository, and the manual project setup that is not |

## Conventions

| Document | What it covers |
| --- | --- |
| [Commit Convention](./commits.md) | Commit message format: Conventional Commits with crate scopes (`feat(lift): …`, `fix(codestream): …`) |
| [Rust Style](./style/rust.md) | Rust style rules — clippy `all` + `pedantic`, the `ndic_core::Result<T>` error contract, and layout conventions |

## Rust 1.98 migration

Start with the summary; the rest are the phase records it draws on.

| Document | What it covers |
| --- | --- |
| [Rust 1.98 Adoption](./rust-198/index.md) | **The summary.** The MSRV decision and its cost on three registries, the measured performance delta with the toolchain effect separated from the code effect, and the evidence that no encoded byte moved |
| [Adoption Notes](./rust-198/adoption-notes.md) | Why the MSRV moved to 1.98 in one step, what downstream consumers on crates.io, PyPI, and npm see, and what each migration phase changed |
| [Capability Probe](./rust-198/capability-probe.md) | What 1.98 actually offers this project, measured by a runnable probe — the exact confirmed signature of every API the migration depends on |
| [Float Drift Inventory](./rust-198/float-drift-inventory.md) | Which exactness tests can observe a float reassociation, and every float arithmetic site in the workspace classified as a candidate |
| [Algebraic Float in the 9/7 DWT](./rust-198/algebraic-97-dwt.md) | What happened when the irreversible CDF 9/7 lifting kernel was converted to algebraic float operations, and why it was reverted |
| [Algebraic Codec Sweep](./rust-198/algebraic-codec-sweep.md) | The sweep of every remaining float site — why the hand-written SIMD module is kept, and where the `ndic-zfp` exactness boundary actually sits |
| [Unsafe Audit](./rust-198/unsafe-audit.md) | Every `unsafe` block in the workspace: what was removed, what is kept and why, and the lint configuration that now makes the next one an argument |
| [Ergonomic Sweep](./rust-198/ergonomic-sweep.md) | The smaller 1.98 APIs applied — `subslice_range` in the codestream reader, `format_into` in the bench reporter, and the three APIs with no site here |

## Decisions

| Document | What it covers |
| --- | --- |
| [ADR 001 — Docs Toolchain](./decisions/adr-001-documentation-toolchain.md) | Why the documentation site is mystmd rooted at `docs/`, deployed to Read the Docs through `build.commands`, and gated on a strict build — plus the two things it deliberately does not do |

## Before you change code

Read the [architecture docs](../architecture/index.md) covering the component you
are touching — they carry the *how and why* behind the code, and a change that
contradicts them needs the page updated in the same PR. What is planned next
lives in the
[open issues](https://github.com/fideus-labs/nd-image-codecs/issues), not in
this tree.
