---
title: Commit Convention
description: nd-image-codecs uses Conventional Commits with crate scopes, so every message names the crate it touches.
---

[Conventional Commits](https://www.conventionalcommits.org/) with crate scopes:

```text
feat(htj2k): implement cleanup-pass MEL coder
fix(codestream): reject PLT lengths past tile-part end
test(lift): property-test 5/3 round-trip on i16 volumes
feat(zfp): 3D fixed-rate block coder
perf(htj2k): AVX2 lane for significance scan
chore(ci): cache the conformance corpus
refactor(zarr): split codec config parsing from execution
docs: add nd-transform architecture doc
```

Scopes: `core`, `htj2k`, `codestream`, `lift`, `zfp`, `zarr`, `cli`, `py`, `ts`,
`bench`, `ci`. Omit the scope for cross-cutting `docs:` commits.

Use `perf(...)` (not `feat`) for performance-only changes — the bench gate report links
PRs to commits by type.

## What reads these

[`CHANGELOG.md`](https://github.com/fideus-labs/nd-image-codecs/blob/main/CHANGELOG.md)
and the body of every GitHub release are generated from these messages by
commitizen, configured in
[`.cz.toml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.cz.toml).
A commit that does not parse as a Conventional Commit appears in neither — so
the subject line is the release note.

The types above map to the changelog's sections: `feat` → ✨ Features, `fix` →
🐛 Bug Fixes, `perf` → ⚡ Performance, `refactor` → ♻️ Refactoring, `docs` →
📚 Documentation, `bench` → 📊 Benchmarks, `test` → 🧪 Tests, `build` →
📦 Build, `ci` → 🤖 Continuous Integration, `style` → 🎨 Style, `chore` →
🧹 Chores, `revert` → ⏪ Reverts. A `!` before the colon (`feat(zfp)!:`) moves
the entry to 💥 Breaking Changes. `release:` is the one type deliberately kept
out of the changelog — those commits carry version bumps, which the release
heading already states.

Commitizen can write the message for you and check one you have written:

```bash
uvx --from commitizen==4.17.0 cz commit  # prompts for type, scope, subject
uvx --from commitizen==4.17.0 cz check --rev-range origin/main..HEAD
```

Neither is required, and neither gates a pull request — see
[Publishing](./publishing.md) for how the changelog is generated.
