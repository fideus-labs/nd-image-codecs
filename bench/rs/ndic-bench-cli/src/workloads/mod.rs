//! The Rust workloads registered with the `ndic-bench` registry.
//!
//! These live in the driver rather than in the codec crates so that the
//! published crates carry no dependency on `ndic-bench-core`, which is
//! `publish = false` and so would block `cargo publish` on every crate that
//! named it — even behind an off-by-default feature. Each module registers
//! its entries with `inventory::submit!`; the driver discovers them at link
//! time and never names them directly.
//!
//! Workloads use only the public API of the crate they measure. Anything that
//! needs crate internals belongs in that crate's own `#[bench]`/`criterion`
//! harness instead, not here.

mod lift;
mod lift_codec;
