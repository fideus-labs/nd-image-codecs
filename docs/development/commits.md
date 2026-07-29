## Commit Convention

[Conventional Commits](https://www.conventionalcommits.org/) with crate scopes:

```
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
