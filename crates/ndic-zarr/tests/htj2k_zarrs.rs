//! `htj2k` as a registered `zarrs` codec: plugin-registry construction, the
//! full nd-lift-ht series (`transpose → nd_lift → htj2k`) round-tripping
//! every supported dtype, sub-chunk reads through the plane-selective
//! partial decoder, fill-value handling, byte-range plans over stored
//! chunks, and acceptance of every builder-emitted configuration.
#![cfg(feature = "zarrs")]

use std::num::NonZeroU64;
use std::sync::Arc;

use serde_json::{Value, json};
use zarrs::array::codec::api::{
    ArrayBytes, ArrayToArrayCodecTraits, ArrayToBytesCodecTraits, Codec, CodecOptions,
};
use zarrs::array::{Array, ArrayBuilder, DataType, Element, ElementOwned, FillValue, data_type};
use zarrs::metadata::v3::MetadataV3;
use zarrs::storage::store::MemoryStore;

use ndic_codestream::range::RangeIndex;
use ndic_codestream::reader::Codestream;
// Using the crate's codec types links its `inventory` submissions (both
// `htj2k` and `nd_lift`) into the test binary.
use ndic_zarr::htj2k_codec::Htj2kCodec;

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

/// The Phase 4 flagship pipeline over a `(z, c, y, x)` array: transpose to
/// `(c, z, y, x)`, lift along z, `htj2k` planes. `kind: "none"` drops the
/// lift (bare `transpose → htj2k`).
fn build_array(store: Arc<MemoryStore>, dtype_name: &str, kind: &str) -> Array<MemoryStore> {
    let (data_type, size) = named_data_type(dtype_name);
    let mut codecs =
        vec![json!({ "name": "transpose", "configuration": { "order": [1, 0, 2, 3] } })];
    if kind != "none" {
        let levels = if kind == "delta" { 0 } else { 2 };
        codecs.push(json!({ "name": "nd_lift", "configuration": {
            "version": "0.1",
            "transforms": [
                { "axis": "z", "dimension": 1, "kind": kind, "levels": levels, "group": 0 }
            ]
        } }));
    }
    codecs.push(json!({ "name": "htj2k", "configuration": {
        "xy_levels": 2, "reversible": true, "progression": "RPCL", "index": true
    } }));

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
    .array_to_bytes_codec(array_to_bytes.expect("htj2k codec"))
    .dimension_names(["z", "c", "y", "x"].into())
    .build(store, "/lift_ht")
    .expect("array builds")
}

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

/// Smooth-in-z data (z-major layout) staying inside both the `nd_lift`
/// overflow budget and the HT datapath's declared-depth ceiling.
fn zwave(i: usize) -> i64 {
    let z = (i / (2 * 8 * 8)) % 8;
    let xy = i % 64;
    i64::try_from(z).unwrap() * 13 + i64::try_from(xy).unwrap() % 7
}

#[test]
fn roundtrips_every_supported_dtype_through_the_series() {
    for kind in ["none", "delta", "haar", "lift53"] {
        roundtrip_dtype::<u8>("uint8", kind, |i| u8::try_from(zwave(i) * 2).unwrap());
        roundtrip_dtype::<i8>("int8", kind, |i| i8::try_from(zwave(i) - 50).unwrap());
        roundtrip_dtype::<u16>("uint16", kind, |i| u16::try_from(zwave(i) * 400).unwrap());
        roundtrip_dtype::<i16>("int16", kind, |i| {
            i16::try_from(zwave(i) * 200 - 15_000).unwrap()
        });
        // 32-bit dtypes ride the depth-flexible path: the *declared* depth
        // follows each plane's actual range, which must stay inside the
        // datapath (≤ ~27 bits after decorrelation growth).
        roundtrip_dtype::<u32>("uint32", kind, |i| {
            u32::try_from(zwave(i) * 500_000).unwrap()
        });
        roundtrip_dtype::<i32>("int32", kind, |i| {
            i32::try_from(zwave(i) * 500_000 - 25_000_000).unwrap()
        });
    }
}

#[test]
fn dtypes_without_an_integer_path_are_refused() {
    // 64-bit dtypes have no HT datapath; the error must be clean, not a
    // panic or a wrap.
    let array = build_array(Arc::new(MemoryStore::new()), "uint64", "lift53");
    let data: Vec<u64> = (0..8 * 2 * 8 * 8)
        .map(|i| u64::try_from(zwave(i)).unwrap())
        .collect();
    assert!(
        array
            .store_array_subset(&array.subset_all(), &data)
            .is_err(),
        "uint64 must refuse to encode"
    );
}

#[test]
fn full_range_int32_is_refused_cleanly() {
    let array = build_array(Arc::new(MemoryStore::new()), "int32", "none");
    let data: Vec<i32> = (0..8 * 2 * 8 * 8).map(|_| i32::MAX).collect();
    assert!(
        array
            .store_array_subset(&array.subset_all(), &data)
            .is_err(),
        "int32 values beyond the HT datapath must refuse to encode"
    );
}

/// Sub-chunk reads exercise the plane-selective partial decoder — both
/// under the lift (whole-chunk request from `NdLiftPartialDecoder`) and
/// bare (a genuine partial read of a few planes).
#[test]
fn sub_chunk_reads_work_through_the_series() {
    for kind in ["none", "delta", "haar", "lift53"] {
        let array = build_array(Arc::new(MemoryStore::new()), "uint16", kind);
        let data: Vec<u16> = (0..8 * 2 * 8 * 8)
            .map(|i| u16::try_from(zwave(i) * 400).unwrap())
            .collect();
        array
            .store_array_subset(&array.subset_all(), &data)
            .expect("store");

        for ranges in [
            vec![3..4, 1..2, 5..6, 6..7],
            vec![2..3, 0..1, 4..5, 0..8],
            vec![2..6, 0..2, 1..3, 1..3],
        ] {
            let got: Vec<u16> = array.retrieve_array_subset(&ranges).expect("retrieve");
            let mut expected = Vec::with_capacity(got.len());
            for z in ranges[0].clone() {
                for c in ranges[1].clone() {
                    for y in ranges[2].clone() {
                        for x in ranges[3].clone() {
                            let i = ((z * 2 + c) * 8 + y) * 8 + x;
                            expected.push(data[usize::try_from(i).unwrap()]);
                        }
                    }
                }
            }
            assert_eq!(got, expected, "{kind}: sub-chunk read of {ranges:?}");
        }
    }
}

#[test]
fn absent_chunks_come_back_as_fill() {
    let array = build_array(Arc::new(MemoryStore::new()), "uint16", "lift53");
    let all: Vec<u16> = array
        .retrieve_array_subset(&array.subset_all())
        .expect("retrieve");
    assert!(all.iter().all(|&v| v == 0));
    // A partial read of an absent chunk too (the partial-decoder path).
    let some: Vec<u16> = array
        .retrieve_array_subset(&[0..1, 0..1, 2..4, 2..4])
        .expect("retrieve");
    assert!(some.iter().all(|&v| v == 0));
}

#[test]
fn unimplemented_configurations_are_refused() {
    for config in [
        json!({ "progression": "SNAKE" }),
        json!({ "xy_levels": 40 }),
        json!({ "tiles": 4 }),
    ] {
        let result = codec_from_json(json!({ "name": "htj2k", "configuration": config }));
        assert!(result.is_err(), "{config} must be refused");
    }
    // Lossy configs construct (arrays carrying them stay openable) but
    // refuse to *encode* until the 9/7 path lands.
    let codec = Htj2kCodec::new_with_configuration(
        &serde_json::from_value(json!({ "reversible": false })).unwrap(),
    )
    .expect("lossy config constructs");
    let shape: Vec<NonZeroU64> = [4, 4]
        .iter()
        .map(|&d| NonZeroU64::new(d).unwrap())
        .collect();
    let (dtype, size) = named_data_type("uint8");
    let result = Arc::new(codec).encode(
        ArrayBytes::from(vec![0u8; 16]),
        &shape,
        &dtype,
        &FillValue::new(vec![0u8; size]),
        &CodecOptions::default(),
    );
    assert!(result.is_err(), "lossy encode must be refused");
}

#[test]
fn configuration_round_trips_through_metadata() {
    use zarrs::array::codec::api::CodecTraits;
    let codec = Htj2kCodec::new_with_configuration(
        &serde_json::from_value(json!({ "xy_levels": 3 })).unwrap(),
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
        json!({ "xy_levels": 3, "reversible": true, "progression": "RPCL", "index": true })
    );
}

#[test]
fn malformed_chunks_error_cleanly() {
    let codec = Arc::new(
        Htj2kCodec::new_with_configuration(&serde_json::from_value(json!({})).unwrap()).unwrap(),
    );
    let shape: Vec<NonZeroU64> = [4, 8, 8]
        .iter()
        .map(|&d| NonZeroU64::new(d).unwrap())
        .collect();
    let (dtype, size) = named_data_type("uint16");
    let fill = FillValue::new(vec![0u8; size]);
    let options = CodecOptions::default();

    // Garbage, an empty buffer, and a truncated valid chunk.
    let good = codec
        .encode(
            ArrayBytes::from(vec![0u8; 4 * 8 * 8 * 2]),
            &shape,
            &dtype,
            &fill,
            &options,
        )
        .expect("encode");
    let mut cases = vec![vec![], vec![0x42; 64]];
    cases.push(good[..good.len() / 2].to_vec());
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

/// The stored chunk bytes serve byte-range plans: a thumbnail plan is ≤ 3
/// coalesced ranges, and executing it (slicing the planned bytes only)
/// decodes to exactly the full plane's downsampled image.
#[test]
fn stored_chunks_serve_thumbnail_plans() {
    let codec = Arc::new(
        Htj2kCodec::new_with_configuration(
            &serde_json::from_value(json!({ "xy_levels": 2 })).unwrap(),
        )
        .unwrap(),
    );
    let shape: Vec<NonZeroU64> = [4, 32, 32]
        .iter()
        .map(|&d| NonZeroU64::new(d).unwrap())
        .collect();
    let (dtype, size) = named_data_type("uint16");
    let fill = FillValue::new(vec![0u8; size]);
    let options = CodecOptions::default();
    let data: Vec<u16> = (0..4 * 32 * 32)
        .map(|i| u16::try_from((i * 7) % 4096).unwrap())
        .collect();
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let chunk = codec
        .encode(ArrayBytes::from(bytes), &shape, &dtype, &fill, &options)
        .expect("encode")
        .into_owned();

    let index = RangeIndex::from_chunk_prefix(&chunk).expect("index");
    let plan = index.thumbnail(8).expect("plan");
    assert!(
        plan.ranges.len() <= 3,
        "standard thumbnail plans stay within 3 ranges: {plan:?}"
    );

    // "Fetch" the planned ranges and decode the plane-0 prefix.
    let header_len = ndic_codestream::container::ChunkHeader::parse(&chunk)
        .unwrap()
        .header_len();
    let fetched: Vec<u8> = plan
        .ranges
        .iter()
        .flat_map(|r| {
            chunk[usize::try_from(r.start).unwrap()..=usize::try_from(r.end).unwrap()]
                .iter()
                .copied()
        })
        .collect();
    // Thumbnail ranges are the contiguous chunk prefix; the plane starts
    // right after the index.
    assert_eq!(plan.ranges[0].start, 0);
    let plane_prefix = &fetched[header_len..];
    let thumb = Codestream::parse_prefix(plane_prefix)
        .expect("prefix parses")
        .decode_to_resolution(plan.max_res)
        .expect("prefix decodes");

    // Compare against the full plane, decoded at the same resolution.
    let (_, ranges) = ndic_zarr::htj2k::plane_ranges(&chunk).unwrap();
    let (off, len) = ranges[0];
    let reference = Codestream::parse(&chunk[off..off + len])
        .unwrap()
        .decode_to_resolution(plan.max_res)
        .unwrap();
    assert_eq!(thumb.width, reference.width);
    assert_eq!(thumb.comps, reference.comps);
    assert_eq!(plan.decoded_size, vec![8, 8]);
}

/// The committed chunk-container micro-fixture: the version-1 byte layout
/// is pinned — re-encoding the same data must reproduce the file
/// byte-for-byte, and the committed bytes must decode to the same data
/// (see `fixtures/codestreams/tiny-chunk-4x8x8.ndht.md`).
#[test]
fn chunk_fixture_is_byte_stable() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/codestreams/tiny-chunk-4x8x8.ndht"
    );
    let committed = std::fs::read(path).expect("committed fixture");
    let shape = [4usize, 8, 8];
    let data: Vec<u8> = (0..shape.iter().product::<usize>())
        .flat_map(|i| u16::try_from(i * 7 % 4096).expect("< 4096").to_ne_bytes())
        .collect();
    let config = ndic_zarr::htj2k::Htj2kConfig {
        xy_levels: 2,
        ..Default::default()
    };
    let encoded =
        ndic_zarr::htj2k::encode_chunk(&data, &shape, ndic_core::SampleType::U16, &config)
            .expect("encode");
    assert_eq!(
        encoded, committed,
        "the htj2k chunk container layout must stay byte-stable \
         (regenerate the fixture only on a deliberate format bump)"
    );
    let decoded = ndic_zarr::htj2k::decode_chunk(&committed, &shape, ndic_core::SampleType::U16)
        .expect("decode");
    assert_eq!(decoded, data);
}

/// Every `htj2k` configuration the cross-language `codec_series` builders
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
            if codec["name"] == "htj2k" {
                codec_from_json(codec.clone()).unwrap_or_else(|err| {
                    panic!(
                        "builder-emitted htj2k configuration must be accepted \
                         (case {:?}): {err}",
                        case["name"]
                    )
                });
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "the fixture matrix must exercise htj2k configs");
}
