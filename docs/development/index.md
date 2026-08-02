# Development

This index is the map of the nd-image-codecs contributor documentation: how to build,
test, benchmark, and release the project, and the conventions every change is held to.
Open the page that matches the task in front of you.

## Working on the project

| Document | What it covers |
| --- | --- |
| [commands.md](./commands.md) | Everyday commands: build and check, test, lint and format, benchmarks, bindings, CLI smoke tests, release, and building the documentation site |
| [benchmarking.md](./benchmarking.md) | Running and adding benchmarks — record layout, named baselines, and the PR regression gate |
| [test-data.md](./test-data.md) | Test data and the conformance corpus: what is vendored, what is downloaded, and where it is cached |
| [publishing.md](./publishing.md) | Publishing a release to crates.io, PyPI, and npm — prerequisites, version-bump locations, and verification |

## Conventions

| Document | What it covers |
| --- | --- |
| [commits.md](./commits.md) | Commit message format: Conventional Commits with crate scopes (`feat(lift): …`, `fix(codestream): …`) |
| [style/rust.md](./style/rust.md) | Rust style rules — clippy `all` + `pedantic`, the `ndic_core::Result<T>` error contract, and layout conventions |

## What to build next

| Section | What's inside | Open |
| --- | --- | --- |
| Roadmap | The six strictly ordered implementation phases: what to build, against which spec clauses and reference implementations, with which tests and benchmarks, and what "done" means | [roadmap/index.md](./roadmap/index.md) |

Deciding what to implement next always starts at
[roadmap/index.md](./roadmap/index.md) — phases are strictly ordered, and a phase's
acceptance criteria gate the next. Read the linked
[architecture docs](../architecture/index.md) before the phase document: the phase doc
tells you *what and when*, the architecture doc *how and why*.
