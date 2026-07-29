# Committed benchmark baselines

Each subdirectory (e.g. `main/`) mirrors a `target/benchmarks/<git-hash>/` records tree
and carries a `manifest.json`:

```json
{
  "name": "main",
  "git_hash": "<hash the records were captured at>",
  "machine": "<runner class, e.g. gha-ubuntu-24.04-8core>",
  "toolchain": "rustc 1.85.0",
  "captured": "YYYY-MM-DD"
}
```

Rules:

- Baselines change **only** via the `bench-baseline-refresh` workflow or an equivalent
  reviewed PR — never as a side effect of a feature PR.
- Records must come from the pinned runner class named in the manifest; mixing machines
  invalidates the σ noise envelope the gate depends on.
- The first baseline is captured at the end of roadmap Phase 1, once the first real
  workloads exist.
