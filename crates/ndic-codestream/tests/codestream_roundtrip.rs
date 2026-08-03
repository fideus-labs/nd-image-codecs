#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)] // test math

//! End-to-end codestream tests: encode -> parse -> decode identity, the
//! TLM/PLT packet index, and partial (by-resolution) decode.

use ndic_codestream::{Codestream, encode_image};
use ndic_core::{CoeffPlane, EncodeParams, SampleType};
use proptest::prelude::*;

fn params(levels: u8) -> EncodeParams {
    EncodeParams {
        xy_levels: levels,
        ..EncodeParams::default()
    }
}

fn synthetic(width: usize, height: usize, seed: u64, dtype: SampleType) -> Vec<i32> {
    let (lo, hi) = match dtype {
        SampleType::U8 => (0i64, 255),
        SampleType::I8 => (-128, 127),
        SampleType::U16 => (0, 65535),
        SampleType::I16 => (-32768, 32767),
        _ => (0, 4095),
    };
    let mut state = seed | 1;
    (0..width * height)
        .map(|i| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            // Mix smooth gradients with noise so blocks vary in energy.
            let x = (i % width) as i64;
            let y = (i / width) as i64;
            let smooth = (x * 3 + y * 7) % (hi - lo + 1);
            let noise = (state >> 40) as i64 % 32;
            (lo + (smooth + noise).clamp(0, hi - lo)) as i32
        })
        .collect()
}

fn roundtrip(width: usize, height: usize, levels: u8, dtype: SampleType, seed: u64) {
    let samples = synthetic(width, height, seed, dtype);
    let plane = CoeffPlane::new(&samples, width, height).unwrap();
    let stream = encode_image(&[plane], dtype, &params(levels)).expect("encode");

    let cs = Codestream::parse(&stream).expect("parse");
    assert_eq!(cs.siz.xsiz as usize, width);
    assert_eq!(cs.cod.decomps, levels);
    assert!(cs.cap.is_some(), "CAP must always be emitted");
    assert!(!cs.tlm.is_empty(), "TLM must always be emitted");

    let dec = cs.decode().expect("decode");
    assert_eq!((dec.width, dec.height), (width, height));
    assert_eq!(
        dec.comps[0], samples,
        "lossless round-trip {width}x{height} L{levels} {dtype:?}"
    );
}

#[test]
fn roundtrips_dtypes_and_geometries() {
    for dtype in [
        SampleType::U8,
        SampleType::I8,
        SampleType::U16,
        SampleType::I16,
    ] {
        roundtrip(64, 64, 2, dtype, 7);
    }
    roundtrip(1, 1, 0, SampleType::U8, 1);
    roundtrip(1, 1, 5, SampleType::U16, 1);
    roundtrip(7, 3, 1, SampleType::U8, 2);
    roundtrip(65, 33, 3, SampleType::U16, 3);
    roundtrip(256, 100, 5, SampleType::U8, 4);
    roundtrip(129, 1, 5, SampleType::I16, 5);
    roundtrip(1, 200, 5, SampleType::U8, 6);
}

#[test]
fn packet_index_matches_walked_offsets() {
    let samples = synthetic(150, 90, 11, SampleType::U16);
    let plane = CoeffPlane::new(&samples, 150, 90).unwrap();
    let stream = encode_image(&[plane], SampleType::U16, &params(3)).expect("encode");
    let cs = Codestream::parse(&stream).expect("parse");

    let spans = cs.packet_index().expect("index");
    assert_eq!(spans.len(), 4, "one packet per resolution");
    // Spans tile the tile-part body exactly.
    let tp = &cs.tile_parts[0];
    assert_eq!(spans[0].offset, tp.body.start);
    let mut expect = tp.body.start;
    for s in &spans {
        assert_eq!(s.offset, expect);
        expect += s.len;
    }
    assert_eq!(expect, tp.body.end);
    // Resolutions ascend under RPCL.
    let rs: Vec<u8> = spans.iter().map(|s| s.res).collect();
    assert_eq!(rs, vec![0, 1, 2, 3]);
}

#[test]
fn partial_decode_matches_wavelet_pyramid() {
    let width = 120;
    let height = 68;
    let levels = 3u8;
    let samples = synthetic(width, height, 21, SampleType::U8);
    let plane = CoeffPlane::new(&samples, width, height).unwrap();
    let stream = encode_image(&[plane], SampleType::U8, &params(levels)).expect("encode");
    let cs = Codestream::parse(&stream).expect("parse");

    // Reference pyramid: forward DWT, then inverse only the kept levels.
    for max_res in 0..=levels {
        let dec = cs.decode_to_resolution(max_res).expect("partial decode");
        let (ew, eh) = ndic_htj2k::dwt::level_dims(width, height, levels - max_res);
        assert_eq!((dec.width, dec.height), (ew, eh), "res {max_res}");

        let mut reference: Vec<i32> = samples.iter().map(|&v| v - 128).collect();
        ndic_htj2k::dwt::forward_53(&mut reference, width, height, levels).unwrap();
        let mut region = vec![0i32; ew * eh];
        for y in 0..eh {
            region[y * ew..(y + 1) * ew].copy_from_slice(&reference[y * width..y * width + ew]);
        }
        ndic_htj2k::dwt::inverse_53(&mut region, ew, eh, max_res).unwrap();
        let shifted: Vec<i32> = region.iter().map(|&v| (v + 128).clamp(0, 255)).collect();
        assert_eq!(dec.comps[0], shifted, "res {max_res}");
    }
}

#[test]
fn multi_component_roundtrip() {
    let width = 40;
    let height = 30;
    let mut planes_data = Vec::new();
    for c in 0..3u64 {
        planes_data.push(synthetic(width, height, 100 + c, SampleType::U8));
    }
    let planes: Vec<CoeffPlane> = planes_data
        .iter()
        .map(|d| CoeffPlane::new(d, width, height).unwrap())
        .collect();
    let stream = encode_image(&planes, SampleType::U8, &params(2)).expect("encode");
    let cs = Codestream::parse(&stream).expect("parse");
    let dec = cs.decode().expect("decode");
    for (c, want) in planes_data.iter().enumerate() {
        assert_eq!(&dec.comps[c], want, "component {c}");
    }
}

#[test]
fn truncated_streams_error_not_panic() {
    let samples = synthetic(32, 32, 5, SampleType::U8);
    let plane = CoeffPlane::new(&samples, 32, 32).unwrap();
    let stream = encode_image(&[plane], SampleType::U8, &params(2)).expect("encode");
    for cut in [1usize, 3, 10, 40, 60, stream.len() / 2, stream.len() - 3] {
        let sub = &stream[..cut];
        // Either parse fails cleanly or decode fails cleanly; no panic.
        if let Ok(cs) = Codestream::parse(sub) {
            let _ = cs.decode();
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn roundtrips_random(
        width in 1usize..=130,
        height in 1usize..=90,
        levels in 0u8..=5,
        seed in any::<u64>(),
    ) {
        roundtrip(width, height, levels, SampleType::U16, seed);
    }
}
