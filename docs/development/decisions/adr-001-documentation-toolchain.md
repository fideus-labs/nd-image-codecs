---
title: ADR 001 — Documentation Toolchain
short_title: ADR 001 — Docs Toolchain
description: Why the documentation site is built with mystmd from a project rooted at docs/, deployed to Read the Docs through build.commands, and gated on a strict build in CI.
date: 2026-08-01
tags:
  - adr
  - documentation
  - tooling
---

# ADR 001 — Documentation Toolchain

**Status:** Accepted · **Date:** 2026-08-01

## Context

The `docs/` tree existed long before it was a website. It was written as
CommonMark for GitHub: 34 pages of architecture, usage, contributor, and roadmap
documentation, heavy on multi-column tables, cross-linked with relative
`./page.md` paths, and read directly in the repository by both humans and coding
agents (`AGENTS.md` routes to it by task).

Publishing it needed a static site generator, a host, and a way to keep the two
honest. The constraints that decided the shape of the answer:

- The existing markdown had to render essentially **as-is**. Rewriting 34 pages
  into another markup dialect would have been the largest part of the work and
  would have made the in-repository reading experience worse.
- The repository is a **Rust monorepo**. Nothing in `docs/` depends on Python,
  and neither does the build, the test suite, or CI's critical path.
- Whatever gates documentation had to be **the same command** a contributor runs
  locally, CI runs on a pull request, and the host runs on deploy — three
  slightly different build invocations is how a site starts failing only on
  deploy.

## Decision 1 — mystmd, not Sphinx or MkDocs

**Decision.** Build the site with [mystmd](https://mystmd.org), pinned in
`docs/package.json`.

**Why.** mystmd parses MyST Markdown, a strict superset of CommonMark, so the
existing pages render without edits — tables, relative links, and fenced code
blocks all carry over, and the source stays readable on GitHub. It resolves
relative `./page.md` links into site routes and fails the build on a
cross-reference it cannot resolve, which is exactly the property the `docs/`
tree needed and never had.

Sphinx would have meant either converting to reStructuredText or adding a
`myst-parser` layer — and in both cases a Python documentation build, a
`requirements.txt`, and a Python toolchain in CI and on the host, in a repository
whose documentation otherwise has no Python dependency at all. MkDocs renders
CommonMark happily but has no equivalent of MyST's typed cross-references and
strict mode; broken internal links stay broken until a reader finds them.

**Consequences.** Documentation tooling is a Node dependency in a Rust
repository. The site's directives (admonitions, figures) are MyST syntax, which
GitHub renders as literal text — so directives are used where the site is the
primary audience (status banners) and avoided in the body prose that people read
on GitHub.

## Decision 2 — `myst.yml` in `docs/`, not the repository root

**Decision.** The MyST project root is `docs/`. The config lives at
[`docs/myst.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/docs/myst.yml).

**Why.** It scopes the site to the documentation tree. A root-level project would
sweep in `README.md`, `AGENTS.md`, `CODE_OF_CONDUCT.md`, and the seven per-crate
`README.md` files — documents written for a different medium and a different
reader, which would then need either exclusion rules or frontmatter they have no
reason to carry.

**Consequences.**

- Read the Docs and the CI job must scope themselves to `docs/`. Every
  `build.commands` entry chains its own `cd docs`, and the CI steps set
  `working-directory: docs`.
- A relative link that escapes the project root (`../../crates/…`) resolves on
  GitHub and 404s on the site. Links from `docs/` into source, benchmarks, or
  scripts are therefore absolute
  `https://github.com/fideus-labs/nd-image-codecs/blob/main/…` URLs, which render
  identically in both places. Links *between* pages inside `docs/` stay relative.
- The per-page "Edit This Page" link still comes out right. mystmd resolves
  `project.github` against the **git** root rather than the MyST project root, so
  the generated URLs already carry the `docs/` prefix
  (`…/edit/main/docs/architecture/zfp.md`), at every depth. `edit_url` does not
  need to be set by hand — verified against a top-level page, an
  `architecture/` page, and a `development/roadmap/` page.

## Decision 3 — Read the Docs via `build.commands`

**Decision.** Deploy to Read the Docs with
[`.readthedocs.yaml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.readthedocs.yaml)
using `build.commands`.

**Why.** Read the Docs has no native mystmd builder. `build.commands` replaces
the default build wholesale — no Sphinx, no MkDocs, no automatic environment
creation — and is mutually exclusive with the `sphinx`, `mkdocs`, and `python`
keys, so the file has none of them. The alternative hosts (GitHub Pages, Netlify)
would have worked too; Read the Docs wins on pull request previews and versioned
builds without any additional configuration.

**Consequences.** Two findings from setting it up cost real time and are written
down here so nobody rediscovers them:

- **Each `build.commands` entry runs in its own shell, starting from the checkout
  root.** A bare `cd docs` on its own line changes the directory of a shell that
  immediately exits. Every entry that needs `docs/` chains its own `cd`, and an
  `export` only survives inside the entry that performs it.
- **`BASE_URL` is mandatory, and must be derived from the environment.** Read the
  Docs never serves a project at the domain root: `latest` publishes under
  `/en/latest/` and a pull request preview under `/en/<pr>/`. Without `BASE_URL`
  mystmd emits root-absolute URLs for the stylesheets, every JavaScript chunk,
  the logo, the favicon, and every inter-page link — all of which 404 under a
  path prefix, leaving an unstyled, un-hydrated page. `READTHEDOCS_VERSION` holds
  the *pull request number* on preview builds, so hardcoding `/en/latest` would
  publish a working `latest` and a broken preview for every pull request.

Creating the project, enabling pull request builds, and managing versions are
account-level operations no file in this repository can perform. They are written
up as a runbook in [Read the Docs deployment](../read-the-docs.md).

## Decision 4 — a pinned `docs/package-lock.json`, not `npm install -g mystmd`

**Decision.** mystmd is a `devDependency` of the private `docs/` package,
installed with `npm ci` from the committed lockfile.

**Why.** `npm install -g mystmd` floats. A contributor, the CI runner, and the
Read the Docs builder would each get whatever version was current the day they
ran it, and a mystmd release that tightened a warning would then break the deploy
of a documentation change that touched nothing. `npm ci` from the lockfile means
one version — 1.10.1 today — in all three places, upgraded deliberately in a pull
request where the strict build proves the upgrade is clean.

**Consequences.** Upgrading mystmd is a lockfile commit, and `npm ci` fails loudly
if `package.json` and `package-lock.json` drift apart. Both CI and Read the Docs
pin Node itself to 22 so the lockfile resolves the same way on both.

## Decision 5 — strict build as the CI gate; external link checking off the gate

**Decision.** `npm run check` — `myst build --html --strict` — is the single
canonical build command. It is what a contributor runs locally, what the `docs`
job in
[`.github/workflows/ci.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.github/workflows/ci.yml)
runs on every pull request, and what `.readthedocs.yaml` runs on deploy. Checking
**external** links is deliberately *not* part of that gate.

**Why.** Strict mode fails on any warning: an unresolved cross-reference, a
duplicate identifier, a missing image, a malformed directive. Every one of those
is caused by the change under review and is fixable by its author, which is what
makes it a fair gate. Because the same script runs in all three places, a green
`docs` check means the Read the Docs build will succeed — the two cannot drift
apart without someone editing `docs/package.json`.

External links fail the opposite way. The documentation cites roughly 90
specification and vendor URLs — ISO, ITU, LLNL, frontiersin, kakadusoftware —
that rate-limit, bot-block, and go offline entirely independently of this
repository. A CI job wrapping them would go red for reasons no contributor can
fix, and a gate that fails for unfixable reasons is worse than no gate: it trains
everyone to ignore it.

**Consequences.** External links are checked by
[`scripts/ci/check-docs-links.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/ci/check-docs-links.py),
which exits non-zero only for a definitively dead target and downgrades blocked,
rate-limited, and unreachable hosts to warnings. It runs monthly from
[`.github/workflows/docs-link-check.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.github/workflows/docs-link-check.yml)
— a **separate, scheduled, non-blocking** workflow, with `workflow_dispatch` for
running it on demand — so link rot is reported on a cadence instead of on the
critical path. See [development commands](../commands.md) for both commands.

## Decision 6 — explicit link text, not MyST's auto-filled titles

**Decision.** Links between pages inside `docs/` carry explicit link text —
`[Overview](./overview.md)`, not `[](./overview.md)`.

**Why.** MyST resolves an empty label to the target page's `title`, which is
convenient and keeps the two in sync, and the first draft of this site used it
throughout. It fails the first constraint in the Context above: these files are
read directly in the repository, and GitHub has no such behavior. It renders
`[](./overview.md)` literally, as `<a href="./overview.md"></a>` — an anchor
with no text. On GitHub the link is invisible: a sentence becomes "see  for the
rules", and a table cell whose only content is the link renders blank.

The auto-filled text is also a poor fit for prose even on the site, because it
inserts the *full* title rather than the `short_title`. "…and so on (see
Byte-Range Access: Thumbnails Without a Smart Server)" is what the reader got.
Explicit text lets a link read as its sentence needs — `byte-range access` in
prose, `Byte-Range Access` in an index table.

**Consequences.** Link text no longer tracks a renamed page title
automatically; a retitled page leaves stale link text behind, and `--strict`
will not catch it because the *target* still resolves. This is the accepted
cost — a stale-but-readable label is a smaller failure than an invisible link,
and page titles here are stable. The rule is recorded in `AGENTS.md` and
[development commands](../commands.md).

## Not in scope / follow-on

Two things a reader might reasonably expect from a documentation site are
deliberately absent. They are follow-on work, not oversights.

### Generated API reference

The MyST site is **narrative only**. rustdoc for the seven crates, the Python
binding's API, and a TypeScript typedoc are not folded into it. CI still builds
rustdoc separately — the `docs` job runs `cargo doc --workspace --no-deps` purely
as a compile check on the doc comments — and publishes nothing.

This is a deliberate scoping call: an API reference is generated from three
different toolchains in three different languages, and each one drags its
toolchain into the documentation build. What integrating it would require, as a
starting point for a future phase:

| Surface | Generator | What it costs the docs build |
| --- | --- | --- |
| Seven Rust crates | `cargo doc --workspace --no-deps` → `target/doc/` | A Rust toolchain in the Read the Docs build image (`build.tools.rust`) and a multi-minute compile, or a separate CI job that uploads the HTML for the docs build to fetch |
| Python binding | `pdoc` / `sphinx-autodoc` on `nd_image_codecs` | A Python toolchain *and* a maturin build, because the module must be importable to be introspected |
| TypeScript binding | `typedoc` | A second npm install; `typedoc-plugin-markdown` could emit MyST-parseable markdown instead of standalone HTML |

The cheapest first step is not integration at all: rustdoc is published
automatically by [docs.rs](https://docs.rs) when the crates hit crates.io (see
[publishing](../publishing.md)), so the site can link out to a reference it does not build.
Integration is only worth it if the reference must be versioned and searched
together with the prose.

One cross-cutting wrinkle to plan for: the `codec_series` builder is implemented
three times — Rust, Python, TypeScript — and a mechanically generated reference
would present it as three unrelated APIs rather than one contract with three
bindings.

### Executable code blocks

Code blocks on this site are **static**. Nothing executes them, and nothing
checks them against the current API.

[Phase 6](../roadmap/phase-6-validation-and-docs.md) owns making them real: its
"usage docs completion" item puts every `docs/usage/*.md` snippet under a docs CI
job, with the `rust,ignore` blocks graduating to tested examples as the APIs
land. That phase now inherits a working documentation pipeline — a strict build,
a CI job, and a deploy — rather than having to invent one; the work left is
executing the snippets, not publishing them.

Until then, the wording on the site has to match reality. The [usage index](../../usage/index.md)
previously asserted that "every code block in these pages is executed by CI
against the current API"; it now states the intent — that the snippets become
CI-executed when Phase 6 lands — so the published site does not advertise a
guarantee the project does not yet make.
