#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)] // test math

//! Malformed-codestream regression tests: every single-byte corruption of a
//! valid stream must produce a clean error (or a wrong image) — never a
//! panic. This covers the marker-validation classes flaged in review:
//! zero tile sizes, out-of-range COD exponents, hostile QCD/Lblock values,
//! truncated SOT segments, and oversized SIZ dimensions.

use ndic_codestream::{Codestream, encode_image};
use ndic_core::{CoeffPlane, EncodeParams, SampleType};

fn small_stream() -> Vec<u8> {
    let samples: Vec<i32> = (0..32 * 20).map(|i| (i * 7) % 256).collect();
    let plane = CoeffPlane::new(&samples, 32, 20).unwrap();
    let params = EncodeParams {
        xy_levels: 2,
        ..EncodeParams::default()
    };
    encode_image(&[plane], SampleType::U8, &params).expect("encode")
}

fn try_decode(bytes: &[u8]) {
    if let Ok(cs) = Codestream::parse(bytes) {
        let _ = cs.packet_index();
        let _ = cs.decode();
    }
}

#[test]
fn single_byte_mutations_never_panic() {
    let stream = small_stream();
    for pos in 0..stream.len() {
        for val in [0x00u8, 0xFF, 0x01, 0x80] {
            if stream[pos] == val {
                continue;
            }
            let mut m = stream.clone();
            m[pos] = val;
            try_decode(&m);
        }
    }
}

#[test]
fn truncations_never_panic() {
    let stream = small_stream();
    for cut in 0..stream.len() {
        try_decode(&stream[..cut]);
    }
}

#[test]
fn hostile_header_values_are_rejected() {
    let stream = small_stream();

    // XTsiz = 0 (SIZ tile width at offset 2 + 2 + 2 + 16 = 24 into SIZ seg).
    let mut m = stream.clone();
    let siz_payload = 6usize; // SOC(2) + marker(2) + Lsiz(2)
    for b in &mut m[siz_payload + 18..siz_payload + 22] {
        *b = 0;
    }
    assert!(Codestream::parse(&m).is_err(), "XTsiz = 0 must be rejected");

    // Huge canvas: Xsiz = Ysiz = 0xFFFFFFFF must not drive an allocation.
    let mut m = stream.clone();
    for b in &mut m[siz_payload + 2..siz_payload + 10] {
        *b = 0xFF;
    }
    if let Ok(cs) = Codestream::parse(&m) {
        assert!(cs.decode().is_err(), "oversized SIZ must not decode");
    }
}
