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
| [Publishing](./publishing.md) | Publishing a release to crates.io, PyPI, and npm — prerequisites, version-bump locations, and verification |
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

## What to build next

| Section | What's inside | Open |
| --- | --- | --- |
| Roadmap | The six strictly ordered implementation phases: what to build, against which spec clauses and reference implementations, with which tests and benchmarks, and what "done" means | [Roadmap](./roadmap/index.md) |

Deciding what to implement next always starts at the
[roadmap](./roadmap/index.md) — phases are strictly ordered, and a phase's
acceptance criteria gate the next. Read the linked
[architecture docs](../architecture/index.md) before the phase document: the phase doc
tells you *what and when*, the architecture doc *how and why*.
