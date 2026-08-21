---
title: Code Style — Rust
short_title: Rust Style
description: 'The Rust conventions every change is held to: pinned toolchain, workspace-inherited clippy lints, error handling through ndic_core::Error, and module layout.'
---

## Toolchain

Rust 1.98+ (pinned in [`rust-toolchain.toml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/rust-toolchain.toml); the MSRV is
set by this workspace, not by a dependency — the codecs adopt 1.98 standard-library
APIs directly), edition 2024. Standard `rustfmt` defaults (no `rustfmt.toml`).

## Clippy

Workspace clippy config in root `Cargo.toml` — all crates inherit via
`[lints] workspace = true`. Clippy `all` + `pedantic` at warn level.
Allowed: `module_name_repetitions`, `must_use_candidate`, `missing_errors_doc`,
`missing_panics_doc`. `unsafe_code` is warn — use it only inside SIMD lane modules
(`core::arch` intrinsics), each `unsafe` block carrying a `// SAFETY:` comment.

## Imports

Three groups separated by blank lines:
1. `std` / `alloc` / `core`
2. External crates
3. `crate::` / local modules

No wildcard imports in production code. `use super::*` only in `#[cfg(test)]`.

## Error Handling

All fallible functions return `ndic_core::Result<T>` (alias for
`Result<T, ndic_core::Error>`). Variants: `InvalidArgument`, `Codestream`
(carries a byte `offset`), `Unsupported`, `Io`. Do not add new error types; put
human context in the `message`. Convert `Option` with
`.ok_or(Error::InvalidArgument { message: ... })`. **The decoder never panics on
malformed input** — fuzzing enforces this.

## Naming

- Functions/methods: `snake_case`
- Types/traits/enum variants: `PascalCase`
- Constants/statics: `SCREAMING_SNAKE_CASE`
- Crate directories: `kebab-case` (e.g., `ndic-htj2k`)
- Source files: `snake_case` (e.g., `block_decoder.rs`)
- SIMD lane modules: `<name>_<isa>.rs` (e.g., `cleanup_avx2.rs`), scalar reference in
  `<name>_scalar.rs`

## Types & Derives

- Small value types: `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]`
- Structs with heap data: `#[derive(Debug, Clone, PartialEq)]`
- Use `#[allow(clippy::cast_possible_truncation)]` only for intentional, comment-justified casts
- `no_std`-capable crates: `#![cfg_attr(not(feature = "std"), no_std)]` with the `std`
  feature on by default

## SIMD & Performance Code

- Scalar reference implementation first; it is the conformance oracle.
- SIMD lanes must be bit-identical to scalar (differential tests enforce).
- Runtime dispatch through a small function-pointer table resolved once (mirroring
  OpenJPH's `ojph_arch` approach); no feature detection in inner loops.
- Every performance-sensitive addition registers a `BenchEntry` in the same PR
  (see [benchmarking](../benchmarking.md)).

## Documentation

- Module-level `//!` doc comments on every module
- Per-item `///` comments with backtick type references (e.g., `[`EncodeParams`]`)
- Spec references in doc comments cite the clause (e.g., "T.814 §7.3.2")

## Tests

- Inline `#[cfg(test)] mod tests { use super::*; }` in every source file
- Descriptive names: `cleanup_roundtrips_random_plane`, `plt_offsets_match_packet_walk`
- Cover happy path, error cases, edge cases (1×1 planes, single-slice z-groups,
  max-bit-depth samples)
- Property tests with `proptest` for every round-trip invariant
