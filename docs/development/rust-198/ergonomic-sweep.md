---
type: report
title: Rust 1.98 Ergonomic Sweep
short_title: Ergonomic Sweep
description: The Phase 06 sweep of the smaller 1.98 APIs — three subslice_range conversions that delete offset arithmetic from the codestream reader, one format_into conversion in the bench reporter, and three APIs with no site in this workspace at all.
created: 2026-08-21
date: 2026-08-21
tags:
  - rust-198
  - ergonomics
  - codestream
  - cli
related:
  - '[[Capability-Probe]]'
  - '[[Rust-198-Adoption-Notes]]'
  - '[[Unsafe-Audit]]'
---

# Rust 1.98 Ergonomic Sweep

Phase 06 of the [Rust 1.98 adoption](./adoption-notes.md) (`[[Rust-198-Adoption-Notes]]`)
spends the four remaining 1.98 additions — `subslice_range` / `substr_range`,
`format_into` with `NumBuffer`, `strip_circumfix`, and `NonZero::from_str_radix` — against
the signatures the
[Capability Probe](./capability-probe.md) (`[[Capability-Probe]]`) confirmed on a real
compiler.

**Four sites converted, and three of the five APIs turned out to have no site in this
workspace at all.** The four that converted are all the same shape: a slice and a
separately computed offset *for* that slice, travelling together and free to disagree.
`subslice_range` collapses each pair into one derivation. The three that did not convert
are recorded below with what was searched, because "there is nothing to convert here" is
only useful if the next reader can see it was looked for.

Nothing about the emitted or accepted bytes changed. That is the whole point of the
`subslice_range` work — see [Verification](#verification) for how it was established
rather than assumed.

## What was applied where

| API | Sites converted | Where |
| --- | --- | --- |
| `<[T]>::subslice_range` | 3 | `reader.rs` main header, `reader.rs` tile-part headers, `bitio.rs` / `packet.rs` packet header |
| `u64::format_into` + `NumBuffer` | 1 | `bench/rs/ndic-bench-cli/src/report.rs` |
| `str::substr_range` | 0 | no site — see [Left alone](#left-alone) |
| `str::strip_circumfix` | 0 | no site |
| `NonZero::from_str_radix` | 0 | no site |

## The marker-segment cursor

[`reader.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-codestream/src/reader.rs)
walks marker segments twice — once for the main header, once per tile-part header — and
both loops carried the same three expressions over the same two numbers:

```rust
if len < 2 || pos + 2 + len > data.len() {
    return Err(err(pos, "marker segment length out of bounds"));
}
let payload = &data[pos + 4..pos + 2 + len];
// … parse payload …
pos += 2 + len;
```

`pos + 2 + len` appears twice, in a bounds check and in a cursor advance, with the slice
that both describe built between them. Nothing ties the three together: an edit to one
compiles fine against the other two, and the failure mode is a reader that resumes one
byte off inside a valid stream — a corrupt decode, not an error.

Both loops now call one helper, and the cursor is read back off the payload itself:

```rust
fn segment(data: &[u8], pos: usize, len: usize) -> Option<(&[u8], usize)> {
    let payload = data.get(pos + 4..pos + 2 + len)?;
    Some((payload, data.subslice_range(payload)?.end))
}
```

Two things fell out that were not the goal:

- **The `len < 2` guard is redundant and always was.** `Lmar` counts its own two length
  bytes, so `len < 2` produces the inverted range `pos + 4 .. pos + 2 + len`, and
  `<[T]>::get` already answers `None` for `start > end`. One `get` covers both malformed
  shapes the two-clause guard covered, with the same error at the same offset. The
  inline test asserts that equivalence at `len` of 0, 1, and 2 rather than leaving it as
  a claim.
- **An `Lmar == 2` segment is legal and now says so.** It is an empty payload — a bare
  `COM` — and the old guard's lower bound was `< 2`, so both forms already accepted it.
  The helper's contract makes it explicit.

## The packet-header reader

[`packet.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-codestream/src/packet.rs)
skips an optional `SOP` segment before reading a packet header, and threaded the size of
that skip through four expressions:

```rust
let mut start = 0usize;
if uses_sop && … { start = 6; }
let mut bb = HeaderBitReader::new_at(&data[start..], offset + start);
// …
let mut header_len = start + bb.terminate();   // twice, on two exit paths
```

`&data[start..]` and `offset + start` describe the same bytes in two ways, and
`terminate()` returned a length in the sub-slice that both callers had to lift back into
`data` by hand — a third coordinate system, on two separate exit paths.

[`HeaderBitReader::new_at`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-codestream/src/bitio.rs)
is replaced by `new_in(parent, sub, base)`, which takes the two slices and asks
`subslice_range` where one sits inside the other. The offset parameter is gone rather
than kept alongside the slice, so a caller can no longer hand over a slice and a
disagreeing offset for it; `terminate()` now reports in the parent's coordinates, and
both call sites lost their `start +`. `base` stays a parameter because it is genuinely
external — it names `data`'s position in a coordinate space `data` is not part of.

A slice that is not part of its stated parent is a caller bug, not malformed input, so
`new_in` degrades to an offset of zero (affecting reported error positions only) behind a
`debug_assert!` instead of panicking. This parser runs on hostile bytes under a fuzzer,
and the no-panic property outranks a sharper diagnostic for a bug that cannot reach it
from input.

`uses_sop` was previously untested — the writer never sets `Scod` bit 1 — so
`sop_prefix_counts_towards_the_header_length` was added to cover both the block-carrying
and the empty-packet exit paths through the new accounting.

(format-into-in-the-bench-reporter)=

## `format_into` in the bench reporter

[`report.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bench/rs/ndic-bench-cli/src/report.rs)
renders three timing cells per record, and one of its four branches is integer:

```rust
format!("{ns} ns")   // →  ns.format_into(&mut NumBuffer::new()), then two push_str
```

Measured A/B on this host, 20 000 reps over 1 000 values spread across the branch's
range, best of seven: **23.4 ns/call → 8.9 ns/call, −62 %**. The rendering is
byte-identical, asserted at every unit boundary by
`ns_rendering_is_unchanged_across_the_unit_boundaries`.

Sixty-two percent of a nanosecond-scale call is not a number anyone will notice in a
bench run — three cells per record against records that take milliseconds to produce.
It is recorded at its true size: the conversion is worth making because the branch is
per-record and the replacement is no harder to read, not because a run got faster.

The other three branches of the same function are `{:.2}` float renderings, and
`format_into` is stable on integer types only. They keep `format!`.

(left-alone)=

## Left alone

| Site | API considered | Why not |
| --- | --- | --- |
| [`range.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-codestream/src/range.rs) — the whole plan builder | `subslice_range` | **The plan builder never holds a slice.** Every bound comes from integers: `PlaneEntry { offset: u64, len: u32, prefix: Vec<u32> }` parsed out of the coefficient-plane index, and `TilePart { offset: usize, body: Range<usize> }` from the reader. `subslice_range` is a pointer-identity lookup against a parent slice, and there is no parent here to ask. |
| `ByteRange::of`'s `start + len - 1` | `subslice_range` | The **representation**, not a derivation: `ByteRange` is end-inclusive because HTTP `Range:` is, and `len()` / `shifted()` are built on that. Out of scope by the phase brief, and unchanged. |
| `reader.rs` `decode_to_resolution` — the `body` buffer and its packet cursor | `subslice_range` | `body` is a **copy**: tile-part bodies concatenated with `extend_from_slice`. `self.data.subslice_range(&body[..])` is `None` by construction, and the cursor lives in `body`'s coordinates, not the codestream's. Exactly the case that must keep its arithmetic. (Inside `packet.rs` the `SOP` sub-slice *is* derived from its parent, which is why that one converted.) |
| [`plans.rs:261`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-cli/src/plans.rs) — `series.strip_prefix('@')` | `strip_circumfix` | A lone prefix strip. `@file` has no suffix, and `strip_circumfix` yields `None` unless **both** affixes match — forcing it here would reject every `@file` argument the CLI accepts. |
| All of `crates/ndic-cli/src/` | `strip_circumfix` | **Zero `strip_suffix` calls in the crate.** There is no prefix/suffix pair anywhere to collapse; the one strip in the CLI is the `@` above. |
| All of `crates/ndic-cli/src/` | `NonZero::from_str_radix` | **No radix parsing and no parse-then-check-zero.** `--chunks`, `--rect`, and `--block` all go through decimal `str::parse`, and none is compared against zero at the CLI boundary — dimension validation lives downstream in `SeriesSpec` and `EncodeParams`, on values that never came from a string. There are no two steps to collapse into one. |
| All of `crates/ndic-cli/src/` | `substr_range` | **Nothing recovers a position in a string it already holds.** `parse_wxh`, `parse_rect`, and `run_series` split and parse without ever needing an index back. `load_pnm` is the one scanner, and it walks `&[u8]` with its own cursor and copies each field into an owned `String` — `substr_range` is `str`-only and pointer-identity, so it would answer `None` for those copies even if a position were wanted. |
| [`commands.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-cli/src/commands.rs) — the `inspect` component / tile-part / packet loops | `format_into` | `println!("{}", n)` **does not allocate** for an integer: `Display` writes straight into the formatter. There is nothing to save, and a `NumBuffer` dance inside a `println!` is the readability regression for no measurable gain that the phase brief rules out. |
| [`zarr_io.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-cli/src/zarr_io.rs) | `format_into` | **No integer is formatted in a loop.** Every `format!` in the file is a one-shot `with_context` on a path, dtype, or shape in an error path. |
| `report.rs` — `fmt_ns`'s three float branches, `fmt_ratio`, `fmt_change` | `format_into` | `format_into` is stable on integer types only; these render `f64` with a precision spec. |
| `bench/rs/ndic-bench-cli/src/main.rs` | `format_into` | The two `format!` calls in the run loop join `&str` module and benchmark names. No integers. |

## Verification

The `subslice_range` conversions are only worth making if they change nothing, so that
was established rather than assumed — from both ends.

**A byte-level capture, before and after.** 65 artifacts over a 301×197 plane (a
deliberately non-power-of-two geometry, 5 levels) and the repository's chunk and
codestream fixtures: full `ndic inspect --packets` dumps, every `index` target
(`thumbnail`, `thumbnail-3d`, `plane`, `region`) at every level and eight pixel budgets,
in both `json` and `curl` form, plus SHA-256 of every encoded stream and every decoded
image including five planned-prefix partial decodes. **All 65 identical**, byte for byte,
across the change.

**The named suites.**

```bash
cargo test -p ndic-codestream --release              # 36 unit + 19 integration, 0 failures
cargo test -p ndic-codestream --test range_plans --release
cargo test -p ndic-cli --test plans_cli --release
cargo test --workspace --release                     # 0 failures
cargo clippy --workspace --all-targets -- -D warnings # clean
cargo fmt --all                                       # no diff
```

**The live path, not just the in-test one.** `plans_cli` runs its own in-process
`Range:` server, so the phase also exercised
[`scripts/range-server.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/range-server.py)
directly — the server the usage documentation tells a reader to start. Over it: an
`ndic index` plan identical to the local-file plan, an `ndic thumbnail` byte-identical to
the local decode, a `curl -H "Range: bytes=$(ndic index --format curl)"` prefix that
`ndic expand --partial` decodes to the same image, and a 3D low-pass preview over the
chunk container identical to its local counterpart. `scripts/ci/check-usage-docs.py`
executes the same examples from the pages themselves; `cli.md`, `index.md`, `rust.md`,
and `thumbnails-and-streaming.md` are green. (`python.md` and `zarr.md` fail in this
environment on `ModuleNotFoundError: No module named 'zarr'` — an absent Python
dependency, on pages this phase does not touch.)

**Four tests added**, all for behaviour that had no coverage before:

- `reader::tests::segment_payload_and_cursor_match_the_declared_length` — the helper
  against the arithmetic it replaced, at every boundary the old `len < 2` guard implied.
- `bitio::tests::new_in_recovers_the_sub_slice_offset` — a sub-slice's offset, the
  parent-relative `terminate()`, and the reported error offset.
- `packet::tests::sop_prefix_counts_towards_the_header_length` — the previously untested
  `Scod` bit 1 path, on both exits.
- `report::tests::ns_rendering_is_unchanged_across_the_unit_boundaries` — the
  `format_into` rendering against the `format!` it replaced, at every unit boundary.

(The count read "three" until Phase 07 tallied the workspace suite: 207 → 211 across this
phase, which is `ndic_codestream`'s unit tests 33 → 36 plus this bench-layer one. The
`format_into` test was described in
[the bench-reporter section](#format-into-in-the-bench-reporter) and simply missing from
this list.)

## What this phase is evidence for

**Three of five APIs had no site, and that is the finding.** Going in, `strip_circumfix`,
`NonZero::from_str_radix`, and `substr_range` were all named against the CLI. All three
turned out to describe code this CLI does not contain: it strips one prefix, parses no
radix, and never asks where a substring sits. An API list drawn from a release
announcement describes what the language gained, not what a codebase does.

**`range.rs` was named and had nothing either — for a reason worth keeping.** The phase
brief expected `subslice_range` there because the plan builder computes byte offsets.
It does, but from an *index of integers*, never from a slice of the chunk. That is the
distinction the API turns on, and it is not visible from the outside: "computes offsets"
and "holds a subslice whose offset it needs" look the same until you read the types.

**The conversions that did land share one shape.** Every one was a slice and a number
describing that slice, travelling together — `(&data[pos+4..pos+2+len], pos + 2 + len)`,
`(&data[start..], offset + start)`. `subslice_range` is narrow, and that pair is the
whole of what it is for. It is worth grepping for the pair rather than for the API.
