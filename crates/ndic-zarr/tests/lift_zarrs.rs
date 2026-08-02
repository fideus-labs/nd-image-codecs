//! `nd_lift` as a registered `zarrs` codec: plugin-registry construction,
//! composition with stock codecs (`transpose → nd_lift → bytes → blosc`),
//! bit-exact round-trips for every supported integer dtype, version refusal,
//! and acceptance of every `nd_lift` configuration the cross-language
//! `codec_series` builders emit.
#![cfg(feature = "zarrs")]

use std::sync::Arc;

use serde_json::{Value, json};
use zarrs::array::codec::api::{ArrayToArrayCodecTraits, ArrayToBytesCodecTraits, Codec};
use zarrs::array::{Array, ArrayBuilder, DataType, Element, ElementOwned, FillValue, data_type};
use zarrs::metadata::v3::MetadataV3;
use zarrs::storage::store::MemoryStore;

// Referencing the codec type links this crate's `inventory` submission into
// the test binary — the same requirement any downstream consumer of the
// registry has.
use ndic_zarr::lift_codec::NdLiftCodec;

/// Instantiate one codec through the zarrs Zarr v3 plugin registry.
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
        "uint32" => (data_type::uint32(), 4),
        "int32" => (data_type::int32(), 4),
        "uint64" => (data_type::uint64(), 8),
        "int64" => (data_type::int64(), 8),
        other => panic!("unexpected dtype {other}"),
    }
}

/// The Phase 2 validation pipeline over a `(z, c, y, x)` chunk: transpose to
/// `(c, z, y, x)`, lift along z, then stock `bytes → blosc`.
fn build_array(store: Arc<MemoryStore>, dtype_name: &str, kind: &str) -> Array<MemoryStore> {
    let (data_type, size) = named_data_type(dtype_name);
    let levels = if kind == "delta" { 0 } else { 2 };
    let codecs = [
        json!({ "name": "transpose", "configuration": { "order": [1, 0, 2, 3] } }),
        json!({ "name": "nd_lift", "configuration": {
            "version": "0.1",
            "transforms": [
                { "axis": "z", "dimension": 1, "kind": kind, "levels": levels, "group": 0 }
            ]
        } }),
        json!({ "name": "bytes", "configuration": { "endian": "little" } }),
        json!({ "name": "blosc", "configuration": {
            "cname": "zstd", "clevel": 5, "shuffle": "shuffle", "typesize": 4, "blocksize": 0
        } }),
    ];
    let mut array_to_array: Vec<Arc<dyn ArrayToArrayCodecTraits>> = Vec::new();
    let mut array_to_bytes: Option<Arc<dyn ArrayToBytesCodecTraits>> = None;
    let mut bytes_to_bytes = Vec::new();
    for metadata in codecs {
        match codec_from_json(metadata).expect("registered codec") {
            Codec::ArrayToArray(codec) => array_to_array.push(codec),
            Codec::ArrayToBytes(codec) => array_to_bytes = Some(codec),
            Codec::BytesToBytes(codec) => bytes_to_bytes.push(codec),
        }
    }
    ArrayBuilder::new(
        vec![8, 2, 8, 8],
        vec![4, 1, 8, 8],
        data_type,
        FillValue::new(vec![0u8; size]),
    )
    .array_to_array_codecs(array_to_array)
    .array_to_bytes_codec(array_to_bytes.expect("bytes codec"))
    .bytes_to_bytes_codecs(bytes_to_bytes)
    .dimension_names(["z", "c", "y", "x"].into())
    .build(store, "/lift")
    .expect("array builds")
}

/// Write a full `(8, 2, 8, 8)` volume, read it back, assert bit-exactness.
fn roundtrip_dtype<T>(dtype_name: &str, kind: &str, values: impl Fn(usize) -> T)
where
    T: Element + ElementOwned + PartialEq + std::fmt::Debug + Clone,
{
    let array = build_array(Arc::new(MemoryStore::new()), dtype_name, kind);
    let data: Vec<T> = (0..8 * 2 * 8 * 8).map(values).collect();
    let subset = array.subset_all();
    array.store_array_subset(&subset, &data).expect("store");
    let back: Vec<T> = array.retrieve_array_subset(&subset).expect("retrieve");
    assert_eq!(back, data, "{dtype_name} × {kind} must round-trip exactly");
}

/// Smooth-in-z data (z-major layout) exercising sign handling while staying
/// inside each dtype's overflow budget.
fn zwave(i: usize) -> i64 {
    let z = (i / (2 * 8 * 8)) % 8;
    let xy = i % 64;
    i64::try_from(z).unwrap() * 13 + i64::try_from(xy).unwrap() % 7
}

#[test]
fn roundtrips_every_integer_dtype_with_blosc() {
    for kind in ["delta", "haar", "lift53"] {
        roundtrip_dtype::<u8>("uint8", kind, |i| u8::try_from(zwave(i) * 2).unwrap());
        roundtrip_dtype::<i8>("int8", kind, |i| i8::try_from(zwave(i) - 50).unwrap());
        roundtrip_dtype::<u16>("uint16", kind, |i| u16::try_from(zwave(i) * 400).unwrap());
        roundtrip_dtype::<i16>("int16", kind, |i| {
            i16::try_from(zwave(i) * 200 - 15_000).unwrap()
        });
        roundtrip_dtype::<u32>("uint32", kind, |i| {
            u32::try_from(zwave(i) * 2_000_000).unwrap()
        });
        roundtrip_dtype::<i32>("int32", kind, |i| {
            i32::try_from(zwave(i) * 2_000_000 - 100_000_000).unwrap()
        });
        roundtrip_dtype::<u64>("uint64", kind, |i| {
            u64::try_from(zwave(i) * 40_000_000_000_000).unwrap()
        });
        roundtrip_dtype::<i64>("int64", kind, |i| {
            zwave(i) * 20_000_000_000_000 - (1_i64 << 40)
        });
    }
}

#[test]
fn constructs_directly_from_configuration() {
    let configuration = serde_json::from_value(json!({
        "version": "0.1",
        "transforms": [
            { "axis": "z", "dimension": 0, "kind": "lift53", "levels": 2, "group": 0 }
        ]
    }))
    .expect("configuration");
    assert!(NdLiftCodec::new_with_configuration(&configuration).is_ok());
}

#[test]
fn unknown_version_is_refused() {
    for version in ["0.2", "1.0", "2.1"] {
        let result = codec_from_json(json!({ "name": "nd_lift", "configuration": {
            "version": version,
            "transforms": [
                { "axis": "z", "dimension": 0, "kind": "delta", "levels": 0, "group": 0 }
            ]
        } }));
        assert!(result.is_err(), "version {version} must be refused");
    }
}

#[test]
fn u32_values_beyond_the_i32_plane_error_cleanly() {
    let array = build_array(Arc::new(MemoryStore::new()), "uint32", "lift53");
    let data: Vec<u32> = (0..8 * 2 * 8 * 8).map(|_| u32::MAX).collect();
    let result = array.store_array_subset(&array.subset_all(), &data);
    assert!(
        result.is_err(),
        "u32 values beyond the i32 coefficient plane must refuse to encode"
    );
}

/// Every `nd_lift` configuration the cross-language `codec_series` builders
/// emit (the committed fixture matrix) must construct through the registry:
/// the Python/TS `NdLift` config classes serialize configs the Rust codec
/// accepts.
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
            continue; // error cases
        };
        for codec in expected {
            if codec["name"] == "nd_lift" {
                codec_from_json(codec.clone()).unwrap_or_else(|err| {
                    panic!(
                        "builder-emitted nd_lift configuration must be accepted \
                         (case {:?}): {err}",
                        case["name"]
                    )
                });
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "the fixture matrix must exercise nd_lift configs");
}
