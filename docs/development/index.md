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
