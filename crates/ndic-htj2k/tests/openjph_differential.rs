//! Differential tests against the `OpenJPH` reference block coder.
//!
//! Consumes binary vectors produced by `scripts/ht-differential.sh` (which
//! builds `scripts/ht_oracle.cpp` against a local `OpenJPH` checkout).
//! For every vector this asserts, in both directions:
//!
//! - our encoder emits **byte-identical** cleanup segments, and
//! - our decoder reproduces the reference decoder's output **bit-exactly**.
//!
//! When the vector file is absent (fresh checkout, no C++ toolchain) the
//! test passes vacuously with a note — CI runs the script first.

use std::io::Read;
use std::path::PathBuf;

use ndic_htj2k::{BlockPasses, decode_block, encode_block};

fn vectors_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.join("target/tools/ht_vectors.bin")
}

fn read_u32(data: &mut impl Read) -> Option<u32> {
    let mut b = [0u8; 4];
    data.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

#[test]
fn matches_openjph_block_coder() {
    let path = vectors_path();
    let Ok(file) = std::fs::File::open(&path) else {
        eprintln!(
            "skipping: {} not found — run scripts/ht-differential.sh first",
            path.display()
        );
        return;
    };
    let mut data = std::io::BufReader::new(file);

    let mut count = 0usize;
    while let Some(width) = read_u32(&mut data) {
        let height = read_u32(&mut data).expect("truncated header");
        let k_max = read_u32(&mut data).expect("truncated header");
        let causal = read_u32(&mut data).expect("truncated header");
        let coded_len = read_u32(&mut data).expect("truncated header") as usize;
        let (width, height) = (width as usize, height as usize);
        let n = width * height;

        let mut samples = vec![0u32; n];
        for s in &mut samples {
            *s = read_u32(&mut data).expect("truncated samples");
        }
        let mut coded = vec![0u8; coded_len];
        data.read_exact(&mut coded).expect("truncated code bytes");
        let mut reference = vec![0u32; n];
        for s in &mut reference {
            *s = read_u32(&mut data).expect("truncated reference");
        }

        let ctx = format!("vector {count}: {width}x{height} K_max={k_max} causal={causal}");

        // Encoder parity: byte-identical segments.
        let ours = encode_block(&samples, width, height, width, k_max - 1)
            .unwrap_or_else(|e| panic!("{ctx}: encode failed: {e}"));
        assert_eq!(ours, coded, "{ctx}: encoded bytes differ");

        // Decoder parity: bit-exact sample reconstruction.
        let mut out = vec![0u32; n];
        decode_block(
            &coded,
            &mut out,
            width,
            height,
            width,
            k_max - 1,
            &BlockPasses::cleanup_only(coded.len()),
            causal != 0,
        )
        .unwrap_or_else(|e| panic!("{ctx}: decode failed: {e}"));
        assert_eq!(out, reference, "{ctx}: decoded samples differ");

        count += 1;
    }
    assert!(count > 0, "vector file existed but held no vectors");
    eprintln!("verified {count} OpenJPH differential vectors");
}
