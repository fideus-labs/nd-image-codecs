//! `numcodecs.delta` as a registered `zarrs` codec: plugin-registry
//! construction, the full nd-delta series (`transpose → numcodecs.delta →
//! bytes → blosc`) round-tripping representative dtypes including wrapping
//! integer deltas, sub-chunk reads, and refusal of unsupported
//! configurations.
#![cfg(feature = "zarrs")]

use std::sync::Arc;

use serde_json::{Value, json};
use zarrs::array::codec::api::{
    ArrayToArrayCodecTraits, ArrayToBytesCodecTraits, BytesToBytesCodecTraits, Codec,
};
use zarrs::array::{Array, ArrayBuilder, DataType, Element, ElementOwned, FillValue, data_type};
use zarrs::metadata::v3::MetadataV3;
use zarrs::storage::store::MemoryStore;

// Using the crate's codec type links its `inventory` submission into the
// test binary.
use ndic_zarr::delta_codec::DeltaCodec;
use ndic_zarr::series::{Axis, Family, SeriesSpec, codec_series};

fn codec_from_json(metadata: Value) -> Result<Codec, zarrs::plugin::PluginCreateError> {
    let metadata: MetadataV3 = serde_json::from_value(metadata).expect("codec metadata");
    Codec::from_metadata(&metadata)
}

fn named_data_type(dtype_name: &str) -> (DataType, usize) {
    match dtype_name {
        "uint8" => (data_type::uint8(), 1),
        "uint16" => (data_type::uint16(), 2),
        "int32" => (data_type::int32(), 4),
        "int64" => (data_type::int64(), 8),
        "float32" => (data_type::float32(), 4),
        other => panic!("unexpected dtype {other}"),
    }
}

/// Build a `(z, y, x)` array from the series builder's own nd-delta
/// pipeline, so the test exercises exactly what the builder emits.
fn build_array(store: Arc<MemoryStore>, dtype_name: &str) -> Array<MemoryStore> {
    let axes = vec![Axis::new(0, "z"), Axis::new(1, "y"), Axis::new(2, "x")];
    let spec = SeriesSpec::new(axes, vec![4, 8, 8], dtype_name, Family::NdDelta);
    let codecs = codec_series(&spec).expect("series builds");

    let (data_type, size) = named_data_type(dtype_name);
    let mut array_to_array: Vec<Arc<dyn ArrayToArrayCodecTraits>> = Vec::new();
    let mut array_to_bytes: Option<Arc<dyn ArrayToBytesCodecTraits>> = None;
    let mut bytes_to_bytes: Vec<Arc<dyn BytesToBytesCodecTraits>> = Vec::new();
    for metadata in codecs {
        match codec_from_json(metadata).expect("registered codec") {
            Codec::ArrayToArray(codec) => array_to_array.push(codec),
            Codec::ArrayToBytes(codec) => array_to_bytes = Some(codec),
            Codec::BytesToBytes(codec) => bytes_to_bytes.push(codec),
        }
    }
    ArrayBuilder::new(
        vec![8, 8, 8],
        vec![4, 8, 8],
        data_type,
        FillValue::new(vec![0u8; size]),
    )
    .array_to_array_codecs(array_to_array)
    .array_to_bytes_codec(array_to_bytes.expect("bytes codec"))
    .bytes_to_bytes_codecs(bytes_to_bytes)
    .dimension_names(["z", "y", "x"].into())
    .build(store, "/nd_delta")
    .expect("array builds")
}

fn roundtrip_dtype<T>(dtype_name: &str, values: impl Fn(usize) -> T)
where
    T: Element + ElementOwned + PartialEq + std::fmt::Debug + Clone,
{
    let array = build_array(Arc::new(MemoryStore::new()), dtype_name);
    let data: Vec<T> = (0..8 * 8 * 8).map(values).collect();
    let subset = array.subset_all();
    array.store_array_subset(&subset, &data).expect("store");
    let back: Vec<T> = array.retrieve_array_subset(&subset).expect("retrieve");
    assert_eq!(back, data, "{dtype_name} must round-trip exactly");
    // The partial-decode path (sub-chunk read under `transpose → delta`).
    let sub = zarrs::array::ArraySubset::new_with_ranges(&[1..3, 2..7, 3..8]);
    let got: Vec<T> = array.retrieve_array_subset(&sub).expect("sub-read");
    let mut want = Vec::new();
    for z in 1..3usize {
        for y in 2..7usize {
            for x in 3..8usize {
                want.push(data[z * 64 + y * 8 + x].clone());
            }
        }
    }
    assert_eq!(got, want, "{dtype_name} sub-chunk read");
}

#[test]
fn roundtrips_representative_dtypes_through_the_series() {
    roundtrip_dtype::<u8>("uint8", |i| u8::try_from(i % 251).unwrap());
    roundtrip_dtype::<u16>("uint16", |i| u16::try_from((i * 7) % 4096).unwrap());
    roundtrip_dtype::<i32>("int32", |i| i32::try_from(i).unwrap() * 1_000 - 250_000);
    roundtrip_dtype::<i64>("int64", |i| i64::try_from(i).unwrap() * 30_000_000_000);
    #[allow(clippy::cast_precision_loss)]
    roundtrip_dtype::<f32>("float32", |i| i as f32);
}

#[test]
fn wrapping_deltas_round_trip() {
    // Alternating extremes force every delta to overflow; NumPy's modular
    // arithmetic (and ours) must still restore the input exactly.
    roundtrip_dtype::<u8>("uint8", |i| if i % 2 == 0 { 0 } else { 255 });
    roundtrip_dtype::<i32>("int32", |i| if i % 2 == 0 { i32::MIN } else { i32::MAX });
}

#[test]
fn constructs_from_the_plugin_registry() {
    let codec = codec_from_json(json!({
        "name": "numcodecs.delta",
        "configuration": { "dtype": "<u2" }
    }))
    .expect("registered");
    assert!(matches!(codec, Codec::ArrayToArray(_)));
    // Explicit astype equal to dtype is the numcodecs default spelled out.
    assert!(
        codec_from_json(json!({
            "name": "numcodecs.delta",
            "configuration": { "dtype": "<u2", "astype": "<u2" }
        }))
        .is_ok()
    );
}

#[test]
fn refuses_unsupported_configurations() {
    for configuration in [
        json!({ "dtype": "<c8" }),
        json!({ "dtype": "<u2", "astype": "<i4" }),
        json!({ "dtype": "<u2", "unknown": 1 }),
        json!({}),
    ] {
        assert!(
            codec_from_json(json!({ "name": "numcodecs.delta", "configuration": configuration }))
                .is_err(),
            "{configuration} must be refused"
        );
    }
}

#[test]
fn metadata_round_trips() {
    use zarrs::array::codec::api::{CodecMetadataOptions, CodecTraits};
    let codec = DeltaCodec::new_with_configuration(
        &serde_json::from_value(json!({ "dtype": "<u2" })).expect("configuration"),
    )
    .expect("codec");
    let configuration = codec
        .configuration(
            zarrs::plugin::ZarrVersion::V3,
            &CodecMetadataOptions::default(),
        )
        .expect("configuration");
    assert_eq!(
        serde_json::to_value(configuration).unwrap(),
        json!({ "dtype": "<u2" }),
        "astype is omitted when unset, matching the builder's output"
    );
}
