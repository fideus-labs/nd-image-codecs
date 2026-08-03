//! Committed micro-fixture tests: the tiny codestreams under
//! `fixtures/codestreams/` must stay byte-stable and decode to their
//! closed-form pixels.

use std::path::PathBuf;

use ndic_codestream::{Codestream, encode_image, jph};
use ndic_core::{CoeffPlane, EncodeParams, SampleType};

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.join("fixtures/codestreams").join(name)
}

/// The fixture image: 8x8, sample (x, y) = (7x + 13y) mod 256.
fn gradient() -> Vec<i32> {
    (0..64)
        .map(|i| (7 * (i % 8) + 13 * (i / 8)) % 256)
        .collect()
}

#[test]
fn tiny_j2c_decodes_and_is_byte_stable() {
    let bytes = std::fs::read(fixture("tiny-gradient-8x8.j2c")).expect("fixture present");
    let cs = Codestream::parse(&bytes).expect("parse");
    assert_eq!((cs.siz.xsiz, cs.siz.ysiz), (8, 8));
    assert_eq!(cs.cod.decomps, 1);
    assert!(cs.cod.is_ht());
    assert!(!cs.tlm.is_empty());
    let spans = cs.packet_index().expect("TLM/PLT index");
    assert_eq!(spans.len(), 2);

    let dec = cs.decode().expect("decode");
    assert_eq!(dec.comps[0], gradient());

    // Byte stability: re-encoding the same image reproduces the file.
    let samples = gradient();
    let plane = CoeffPlane::new(&samples, 8, 8).unwrap();
    let params = EncodeParams {
        xy_levels: 1,
        ..EncodeParams::default()
    };
    let reencoded = encode_image(&[plane], SampleType::U8, &params).expect("encode");
    assert_eq!(reencoded, bytes, "fixture must stay byte-stable");
}

#[test]
fn tiny_jph_decodes_and_is_byte_stable() {
    let bytes = std::fs::read(fixture("tiny-gradient-8x8.jph")).expect("fixture present");
    let parsed = jph::parse(&bytes).expect("box walk");
    let h = parsed.header.expect("ihdr");
    assert_eq!((h.width, h.height, h.num_comps, h.bpc), (8, 8, 1, 7));

    let cs_bytes = jph::unwrap_codestream(&bytes).expect("unwrap");
    let dec = Codestream::parse(cs_bytes)
        .expect("parse")
        .decode()
        .expect("decode");
    assert_eq!(dec.comps[0], gradient());

    // Byte stability through the box writer.
    let samples = gradient();
    let plane = CoeffPlane::new(&samples, 8, 8).unwrap();
    let params = EncodeParams {
        xy_levels: 1,
        ..EncodeParams::default()
    };
    let stream = encode_image(&[plane], SampleType::U8, &params).expect("encode");
    let rewrapped = jph::wrap(&stream, 8, 8, &[(8, false)]);
    assert_eq!(rewrapped, bytes, "fixture must stay byte-stable");
}
