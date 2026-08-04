# Benchmark viewer

A static site that renders `BenchRecord` JSON files: per-benchmark history across
commits, config overlays (scalar vs SIMD lanes, z-levels), baseline markers, and —
once the reference lanes are wired up — comparisons against OpenJPH and
imagecodecs plus rate–distortion plots.

Generated into `target/benchmarks/site/` by the driver (`ndic-bench site`);
published by the nightly workflow. Implementation: plain HTML + a small ES
module in [`src/`](./src/) reading record JSON over `fetch` — no build step required to
view locally:

```sh
cargo run -p ndic-bench-cli --release -- run
python3 -m http.server -d target/benchmarks/site
```
