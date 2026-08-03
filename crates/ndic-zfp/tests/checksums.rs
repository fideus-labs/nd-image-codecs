//! The committed `fixtures/zfp/checksums.json` matrix reproduced
//! bit-exactly, and the pinned chunk fixture's byte stability.
//!
//! C-bitstream parity itself is carried by `zfp-rs` (which reproduces the
//! upstream LLNL test suite's checksums against `zfp-sys` in its own CI)
//! and by the `imagecodecs` differential lane in
//! `bindings/python/nd-image-codecs/tests/test_nd_zfp_roundtrip.py`; this
//! matrix pins the exact chunk streams `nd_zfp` emits so a byte-level
//! change can never land silently.
#![cfg(feature = "serde")]

mod common;

use serde_json::Value;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../fixtures/zfp/{name}"))
}

#[test]
fn checksum_matrix_is_reproduced_bit_exactly() {
    let committed: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("checksums.json")).expect("fixture"))
            .expect("valid JSON");
    let committed = committed["cases"].as_array().expect("cases");
    let cases = common::matrix_cases();
    assert_eq!(
        committed.len(),
        cases.len(),
        "matrix size drifted from the committed fixture; regenerate deliberately"
    );
    for (case, (shape, dtype, config)) in committed.iter().zip(cases) {
        let n: usize = shape.iter().product();
        let chunk = common::ramp_chunk(n, dtype);
        let zdtype = ndic_zfp::ZfpDtype::from_zarr_name(dtype).expect("matrix dtype");
        let stream = ndic_zfp::encode_chunk(&chunk, &shape, zdtype, &config).expect("encode");
        let label = format!("{shape:?} {dtype} {config:?}");
        assert_eq!(
            case["configuration"],
            serde_json::to_value(&config).expect("config"),
            "case order drifted: {label}"
        );
        assert_eq!(
            case["bytes"],
            stream.len(),
            "stream length drifted: {label}"
        );
        assert_eq!(
            case["fnv1a64"].as_str().expect("checksum"),
            format!("{:#018x}", common::fnv1a64(&stream)),
            "stream bytes drifted: {label}"
        );
        // Reversible cases must also round-trip bit-exactly.
        if config.mode == "reversible" {
            let decoded = ndic_zfp::decode_chunk(&stream, &shape, zdtype, &config).expect("decode");
            assert_eq!(decoded, chunk, "reversible round-trip broke: {label}");
        }
    }
}

#[test]
fn chunk_fixture_is_byte_stable() {
    let committed = std::fs::read(fixture("tiny-chunk-4x8x8-rate8.zfp")).expect("fixture");
    let config = ndic_zfp::NdZfpConfig {
        mode: "fixed_rate".into(),
        rate: Some(8.0),
        dims: 3,
        ..Default::default()
    };
    let encoded = ndic_zfp::encode_chunk(
        &common::tiny_chunk_f32(),
        &[4, 8, 8],
        ndic_zfp::ZfpDtype::F32,
        &config,
    )
    .expect("encode");
    assert_eq!(
        encoded, committed,
        "the nd_zfp stream layout must stay byte-stable \
         (regenerate the fixture only on a deliberate format bump)"
    );
    // The committed bytes decode (fixed-rate is lossy; geometry only).
    let decoded = ndic_zfp::decode_chunk(&committed, &[4, 8, 8], ndic_zfp::ZfpDtype::F32, &config)
        .expect("decode");
    assert_eq!(decoded.len(), 4 * 8 * 8 * 4);
}
