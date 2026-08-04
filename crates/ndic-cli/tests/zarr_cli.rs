//! `ndic zarr write` / `ndic zarr read` round-trips through a filesystem
//! store for each codec family — the zarrs corner of the cross-ecosystem
//! validation matrix, exercised end to end through the CLI surface the
//! orchestrator drives.
#![cfg(feature = "zarr")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn ndic() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ndic"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean tmp dir");
    }
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    dir
}

/// Deterministic little-endian uint16 volume (the fixtures' ramp pattern).
fn u16_ramp(len: usize) -> Vec<u8> {
    (0..len)
        .flat_map(|i| u16::try_from((i * 7) % 4096).unwrap().to_le_bytes())
        .collect()
}

fn roundtrip(dir: &Path, spec: &str, input: &[u8]) {
    let spec_path = dir.join("spec.json");
    let input_path = dir.join("input.raw");
    let output_path = dir.join("output.raw");
    let store = dir.join("store.zarr");
    std::fs::write(&spec_path, spec).expect("write spec");
    std::fs::write(&input_path, input).expect("write input");

    let status = ndic()
        .args(["zarr", "write", "--store"])
        .arg(&store)
        .arg("--spec")
        .arg(&spec_path)
        .arg("--input")
        .arg(&input_path)
        .status()
        .expect("run ndic zarr write");
    assert!(status.success(), "write failed");
    assert!(store.join("zarr.json").is_file(), "metadata written");

    let status = ndic()
        .args(["zarr", "read", "--store"])
        .arg(&store)
        .arg("--output")
        .arg(&output_path)
        .status()
        .expect("run ndic zarr read");
    assert!(status.success(), "read failed");
    let back = std::fs::read(&output_path).expect("read output");
    assert_eq!(back, input, "store must round-trip byte-exactly");
}

#[test]
fn nd_delta_store_roundtrips() {
    let dir = tmp("nd-delta");
    let spec = r#"{
        "name": "cli-nd-delta",
        "shape": [8, 16, 16],
        "axes": ["z", "y", "x"],
        "chunk_shape": [4, 16, 16],
        "dtype": "uint16",
        "family": "nd-delta"
    }"#;
    roundtrip(&dir, spec, &u16_ramp(8 * 16 * 16));
}

#[test]
fn nd_lift_ht_store_roundtrips() {
    let dir = tmp("nd-lift-ht");
    let spec = r#"{
        "name": "cli-nd-lift-ht",
        "shape": [8, 2, 16, 16],
        "axes": ["z", "c", "y", "x"],
        "chunk_shape": [4, 1, 16, 16],
        "dtype": "uint16",
        "family": "nd-lift-ht",
        "options": { "xy_levels": 2 }
    }"#;
    roundtrip(&dir, spec, &u16_ramp(8 * 2 * 16 * 16));
}

#[test]
fn nd_zfp_store_roundtrips() {
    let dir = tmp("nd-zfp");
    let spec = r#"{
        "name": "cli-nd-zfp",
        "shape": [8, 16, 16],
        "axes": ["z", "y", "x"],
        "chunk_shape": [4, 16, 16],
        "dtype": "float32",
        "family": "nd-zfp"
    }"#;
    #[allow(clippy::cast_precision_loss)]
    let input: Vec<u8> = (0..8 * 16 * 16)
        .flat_map(|i| (((i * 7) % 4096) as f32 / 3.0).to_le_bytes())
        .collect();
    roundtrip(&dir, spec, &input);
}

#[test]
fn write_refuses_a_short_input() {
    let dir = tmp("short-input");
    let spec_path = dir.join("spec.json");
    let input_path = dir.join("input.raw");
    std::fs::write(
        &spec_path,
        r#"{ "shape": [4, 4], "axes": ["y", "x"], "chunk_shape": [4, 4],
             "dtype": "uint16", "family": "nd-zfp" }"#,
    )
    .expect("write spec");
    std::fs::write(&input_path, [0u8; 3]).expect("write input");
    let output = ndic()
        .args(["zarr", "write", "--store"])
        .arg(dir.join("store.zarr"))
        .arg("--spec")
        .arg(&spec_path)
        .arg("--input")
        .arg(&input_path)
        .output()
        .expect("run ndic zarr write");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("needs"),
        "diagnostic names the size: {stderr}"
    );
}
