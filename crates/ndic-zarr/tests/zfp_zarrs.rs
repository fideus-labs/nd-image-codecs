//! `nd_zfp` as a registered `zarrs` codec: plugin-registry construction,
//! the full nd-zfp series (`transpose → nd_zfp`) round-tripping every
//! supported dtype reversibly, brick-selective sub-chunk reads through the
//! fixed-rate partial decoder, fill-value handling, malformed-chunk
//! safety, and acceptance of every builder-emitted configuration.
#![cfg(feature = "zarrs")]
#![allow(clippy::cast_precision_loss)] // test fixtures cast small ints to floats
#![allow(clippy::needless_pass_by_value)] // JSON configs are built at call sites

use std::num::NonZeroU64;
use std::sync::Arc;

use serde_json::{Value, json};
use zarrs::array::codec::api::{
    ArrayBytes, ArrayToArrayCodecTraits, ArrayToBytesCodecTraits, Codec, CodecOptions,
};
use zarrs::array::{Array, ArrayBuilder, DataType, Element, ElementOwned, FillValue, data_type};
use zarrs::metadata::v3::MetadataV3;
use zarrs::storage::store::MemoryStore;

// Using the crate's codec type links its `inventory` submission into the
// test binary.
use ndic_zarr::zfp_codec::NdZfpCodec;

fn codec_from_json(metadata: Value) -> Result<Codec, zarrs::plugin::PluginCreateError> {
    let metadata: MetadataV3 = serde_json::from_value(metadata).expect("codec metadata");
    Codec::from_metadata(&metadata)
}

fn named_data_type(dtype_name: &str) -> (DataType, usize) {
    match dtype_name {
        "uint8" => (data_type::uint8(), 1),
        "int8" => (data_type::int8(), 1),
        "uint16" => (data_type::uint16(), 2),
        "int16" => (data_type::int16(), 2),
        "int32" => (data_type::int32(), 4),
        "int64" => (data_type::int64(), 8),
        "uint32" => (data_type::uint32(), 4),
        "float32" => (data_type::float32(), 4),
        "float64" => (data_type::float64(), 8),
        other => panic!("unexpected dtype {other}"),
    }
}

/// The Phase 5 flagship pipeline over a `(z, c, y, x)` array: transpose to
/// `(c, z, y, x)`, ZFP over the three non-singleton chunk dims.
/// `zfp_config` is the `nd_zfp` configuration object.
fn build_array(store: Arc<MemoryStore>, dtype_name: &str, zfp_config: Value) -> Array<MemoryStore> {
    let (data_type, size) = named_data_type(dtype_name);
    let codecs = vec![
        json!({ "name": "transpose", "configuration": { "order": [1, 0, 2, 3] } }),
        json!({ "name": "nd_zfp", "configuration": zfp_config }),
    ];

    let mut array_to_array: Vec<Arc<dyn ArrayToArrayCodecTraits>> = Vec::new();
    let mut array_to_bytes: Option<Arc<dyn ArrayToBytesCodecTraits>> = None;
    for metadata in codecs {
        match codec_from_json(metadata).expect("registered codec") {
            Codec::ArrayToArray(codec) => array_to_array.push(codec),
            Codec::ArrayToBytes(codec) => array_to_bytes = Some(codec),
            Codec::BytesToBytes(_) => unreachable!(),
        }
    }
    ArrayBuilder::new(
        vec![8, 2, 8, 8],
        vec![4, 1, 8, 8],
        data_type,
        FillValue::new(vec![0u8; size]),
    )
    .array_to_array_codecs(array_to_array)
    .array_to_bytes_codec(array_to_bytes.expect("nd_zfp codec"))
    .dimension_names(["z", "c", "y", "x"].into())
    .build(store, "/nd_zfp")
    .expect("array builds")
}

fn reversible_dims3() -> Value {
    json!({ "mode": "reversible", "dims": 3 })
}

fn roundtrip_dtype<T>(dtype_name: &str, values: impl Fn(usize) -> T)
where
    T: Element + ElementOwned + PartialEq + std::fmt::Debug + Clone,
{
    let array = build_array(Arc::new(MemoryStore::new()), dtype_name, reversible_dims3());
    let data: Vec<T> = (0..8 * 2 * 8 * 8).map(values).collect();
    let subset = array.subset_all();
    array.store_array_subset(&subset, &data).expect("store");
    let back: Vec<T> = array.retrieve_array_subset(&subset).expect("retrieve");
    assert_eq!(back, data, "{dtype_name} must round-trip exactly");
}

/// Smooth-in-z data (z-major layout).
fn zwave(i: usize) -> i64 {
    let z = (i / (2 * 8 * 8)) % 8;
    let xy = i % 64;
    i64::try_from(z).unwrap() * 13 + i64::try_from(xy).unwrap() % 7
}

#[test]
fn roundtrips_every_supported_dtype_reversibly() {
    roundtrip_dtype::<u8>("uint8", |i| u8::try_from(zwave(i) * 2).unwrap());
    roundtrip_dtype::<i8>("int8", |i| i8::try_from(zwave(i) - 50).unwrap());
    roundtrip_dtype::<u16>("uint16", |i| u16::try_from(zwave(i) * 400).unwrap());
    roundtrip_dtype::<i16>("int16", |i| i16::try_from(zwave(i) * 200 - 15_000).unwrap());
    roundtrip_dtype::<i32>("int32", |i| {
        i32::try_from(zwave(i) * 500_000 - 25_000_000).unwrap()
    });
    roundtrip_dtype::<i64>("int64", |i| zwave(i) * 50_000_000_000 - 44);
    roundtrip_dtype::<f32>("float32", |i| zwave(i) as f32 / 3.0);
    roundtrip_dtype::<f64>("float64", |i| zwave(i) as f64 / 3.0 - 11.5);
}

#[test]
fn unsigned_32bit_has_no_zfp_path() {
    // ZFP codes i32/i64/f32/f64 (plus promoted narrow ints); uint32 must
    // refuse to encode cleanly.
    let array = build_array(Arc::new(MemoryStore::new()), "uint32", reversible_dims3());
    let data: Vec<u32> = (0..8 * 2 * 8 * 8)
        .map(|i| u32::try_from(zwave(i)).unwrap())
        .collect();
    assert!(
        array
            .store_array_subset(&array.subset_all(), &data)
            .is_err(),
        "uint32 must refuse to encode"
    );
}

#[test]
fn fixed_rate_bounds_the_error_on_smooth_data() {
    let array = build_array(
        Arc::new(MemoryStore::new()),
        "float32",
        json!({ "mode": "fixed_rate", "rate": 16.0, "dims": 3 }),
    );
    let data: Vec<f32> = (0..8 * 2 * 8 * 8).map(|i| zwave(i) as f32 / 3.0).collect();
    let subset = array.subset_all();
    array.store_array_subset(&subset, &data).expect("store");
    let back: Vec<f32> = array.retrieve_array_subset(&subset).expect("retrieve");
    let worst = data
        .iter()
        .zip(&back)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 0.5,
        "16 bits/value on smooth data must stay accurate, worst error {worst}"
    );
}

/// Sub-chunk reads exercise the partial decoder: in fixed-rate mode the
/// brick-selective path, in reversible mode the whole-chunk fallback.
/// Both must agree exactly with the full decode.
#[test]
fn sub_chunk_reads_work_through_the_series() {
    for config in [
        reversible_dims3(),
        json!({ "mode": "fixed_rate", "rate": 12.0, "dims": 3 }),
    ] {
        let array = build_array(Arc::new(MemoryStore::new()), "float32", config.clone());
        let data: Vec<f32> = (0..8 * 2 * 8 * 8).map(|i| zwave(i) as f32 / 3.0).collect();
        array
            .store_array_subset(&array.subset_all(), &data)
            .expect("store");
        // Whatever the mode did to the data, sub-chunk reads must match
        // the full decode.
        let full: Vec<f32> = array
            .retrieve_array_subset(&array.subset_all())
            .expect("retrieve");

        for ranges in [
            vec![3..4, 1..2, 5..6, 6..7],
            vec![2..3, 0..1, 4..5, 0..8],
            vec![2..6, 0..2, 1..3, 1..3],
            vec![0..1, 0..1, 3..8, 2..7],
        ] {
            let got: Vec<f32> = array.retrieve_array_subset(&ranges).expect("retrieve");
            let mut expected = Vec::with_capacity(got.len());
            for z in ranges[0].clone() {
                for c in ranges[1].clone() {
                    for y in ranges[2].clone() {
                        for x in ranges[3].clone() {
                            let i = ((z * 2 + c) * 8 + y) * 8 + x;
                            expected.push(full[usize::try_from(i).unwrap()]);
                        }
                    }
                }
            }
            assert_eq!(got, expected, "{config}: sub-chunk read of {ranges:?}");
        }
    }
}

#[test]
fn absent_chunks_come_back_as_fill() {
    let array = build_array(
        Arc::new(MemoryStore::new()),
        "float32",
        json!({ "mode": "fixed_rate", "rate": 8.0, "dims": 3 }),
    );
    let all: Vec<f32> = array
        .retrieve_array_subset(&array.subset_all())
        .expect("retrieve");
    assert!(all.iter().all(|&v| v == 0.0));
    // A partial read of an absent chunk too (the brick-selective path).
    let some: Vec<f32> = array
        .retrieve_array_subset(&[0..1, 0..1, 2..4, 2..4])
        .expect("retrieve");
    assert!(some.iter().all(|&v| v == 0.0));
}

#[test]
fn invalid_configurations_are_refused() {
    for config in [
        json!({ "mode": "zstd" }),
        json!({ "mode": "fixed_rate" }),
        json!({ "mode": "reversible", "rate": 8.0 }),
        json!({ "mode": "fixed_rate", "rate": 8.0, "tolerance": 0.5 }),
        json!({ "mode": "fixed_precision", "precision": 65 }),
        json!({ "dims": 5 }),
        json!({ "level": 5 }),
    ] {
        let result = codec_from_json(json!({ "name": "nd_zfp", "configuration": config }));
        assert!(result.is_err(), "{config} must be refused");
    }
}

#[test]
fn configuration_round_trips_through_metadata() {
    use zarrs::array::codec::api::CodecTraits;
    let codec = NdZfpCodec::new_with_configuration(
        &serde_json::from_value(json!({ "mode": "fixed_rate", "rate": 8.0 })).unwrap(),
    )
    .unwrap();
    let configuration = codec
        .configuration(
            zarrs::plugin::ZarrVersion::V3,
            &zarrs::array::codec::api::CodecMetadataOptions::default(),
        )
        .expect("configuration");
    let json = serde_json::to_value(&configuration).unwrap();
    assert_eq!(
        json,
        json!({ "mode": "fixed_rate", "rate": 8.0, "dims": 3 })
    );

    // The defaults: reversible over 3 dims, no mode parameters emitted.
    let codec =
        NdZfpCodec::new_with_configuration(&serde_json::from_value(json!({})).unwrap()).unwrap();
    let configuration = codec
        .configuration(
            zarrs::plugin::ZarrVersion::V3,
            &zarrs::array::codec::api::CodecMetadataOptions::default(),
        )
        .expect("configuration");
    let json = serde_json::to_value(&configuration).unwrap();
    assert_eq!(json, json!({ "mode": "reversible", "dims": 3 }));
}

#[test]
fn malformed_chunks_error_cleanly() {
    let codec = Arc::new(
        NdZfpCodec::new_with_configuration(
            &serde_json::from_value(json!({ "mode": "fixed_rate", "rate": 8.0, "dims": 3 }))
                .unwrap(),
        )
        .unwrap(),
    );
    let shape: Vec<NonZeroU64> = [4, 8, 8]
        .iter()
        .map(|&d| NonZeroU64::new(d).unwrap())
        .collect();
    let (dtype, size) = named_data_type("float32");
    let fill = FillValue::new(vec![0u8; size]);
    let options = CodecOptions::default();

    let good = codec
        .encode(
            ArrayBytes::from(vec![0u8; 4 * 8 * 8 * 4]),
            &shape,
            &dtype,
            &fill,
            &options,
        )
        .expect("encode");
    // Garbage, an empty buffer, a truncated valid chunk, and a padded one.
    let cases = vec![
        vec![],
        vec![0x42; 64],
        good[..good.len() / 2].to_vec(),
        [good.to_vec(), vec![0u8; 64]].concat(),
    ];
    for case in cases {
        let result = codec.decode(
            zarrs::array::codec::api::ArrayBytesRaw::from(case),
            &shape,
            &dtype,
            &fill,
            &options,
        );
        assert!(result.is_err(), "malformed chunk must error, not panic");
    }
}

/// Every `nd_zfp` configuration the cross-language `codec_series` builders
/// emit must construct through the registry.
#[test]
fn accepts_every_series_builder_configuration() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/codec-series/matrix.json"
    );
    let matrix: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture matrix"))
            .expect("valid JSON");
    let mut seen = 0;
    for case in matrix["cases"].as_array().expect("cases") {
        let Some(expected) = case["expected"].as_array() else {
            continue;
        };
        for codec in expected {
            if codec["name"] == "nd_zfp" {
                codec_from_json(codec.clone()).unwrap_or_else(|err| {
                    panic!(
                        "builder-emitted nd_zfp configuration must be accepted \
                         (case {:?}): {err}",
                        case["name"]
                    )
                });
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "the fixture matrix must exercise nd_zfp configs");
}
