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
    let (_, size) = named_data_type(dtype_name);
    build_array_with_fill(store, dtype_name, kind, FillValue::new(vec![0u8; size]))
}

fn build_array_with_fill(
    store: Arc<MemoryStore>,
    dtype_name: &str,
    kind: &str,
    fill_value: FillValue,
) -> Array<MemoryStore> {
    let (data_type, size) = named_data_type(dtype_name);
    let levels = if kind == "delta" { 0 } else { 2 };
    // Blosc sees the *encoded* array: `nd_lift` hands it the widened
    // coefficient plane, so the shuffle must group by the plane's element
    // width (8 bytes for 64-bit input, 4 for everything else), not the input
    // dtype's. A fixed typesize would shuffle 64-bit lanes as pairs of
    // 32-bit ones and make the series unrepresentative of what it validates.
    let typesize = if size == 8 { 8 } else { 4 };
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
            "cname": "zstd", "clevel": 5, "shuffle": "shuffle",
            "typesize": typesize, "blocksize": 0
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
    ArrayBuilder::new(vec![8, 2, 8, 8], vec![4, 1, 8, 8], data_type, fill_value)
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

/// How many elements a `[z, c, y, x]` range list covers.
fn element_count(ranges: &[std::ops::Range<u64>]) -> usize {
    ranges
        .iter()
        .map(|r| usize::try_from(r.end - r.start).unwrap())
        .product()
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

#[test]
fn coefficients_that_do_not_narrow_error_cleanly() {
    use zarrs::array::FillValue;
    use zarrs::array::codec::api::{ArrayBytes, ArrayToArrayCodecTraits, CodecOptions};

    let codec = std::sync::Arc::new(
        NdLiftCodec::new_with_configuration(
            &serde_json::from_value(json!({ "version": "0.1", "transforms": [] })).unwrap(),
        )
        .unwrap(),
    );
    let shape = [std::num::NonZeroU64::new(4).unwrap()];
    let fill = FillValue::new(vec![0u8; 2]);
    let options = CodecOptions::default();
    // An int32 coefficient plane holding 70 000 cannot narrow back to uint16.
    let coeffs: Vec<i32> = vec![1, 70_000, 2, 3];
    let bytes = ArrayBytes::from(bytemuck::cast_slice::<i32, u8>(&coeffs).to_vec());
    let (dtype, _) = named_data_type("uint16");
    let err = codec
        .decode(bytes, &shape, &dtype, &fill, &options)
        .expect_err("out-of-range coefficients must refuse to narrow");
    assert!(
        err.to_string().contains("does not narrow back to u16"),
        "unexpected error: {err}"
    );
}

/// Reading a region smaller than a chunk goes through the codec's partial
/// decoder. Lifting couples samples, so that decoder inverts the whole chunk
/// and slices — and it has to own that itself: the codec chain's generic
/// cache is sized from the wrong side of a shape-changing neighbour, so
/// leaning on it made every sub-chunk read through `transpose → nd_lift`
/// fail with `IncompatibleIndexer`.
#[test]
fn sub_chunk_reads_work_under_transpose() {
    for kind in ["delta", "haar", "lift53"] {
        let array = build_array(Arc::new(MemoryStore::new()), "uint16", kind);
        let data: Vec<u16> = (0..8 * 2 * 8 * 8)
            .map(|i| u16::try_from(zwave(i) * 400).unwrap())
            .collect();
        array
            .store_array_subset(&array.subset_all(), &data)
            .expect("store");

        // One voxel, a row, and a slab that straddles the z-chunk boundary —
        // none of them chunk-aligned in the transformed axis.
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

/// `encoded_fill_value` widens the fill value instead of lifting a filled
/// chunk, which no scalar could represent for a non-zero fill (a constant
/// chunk lifts to `[v, 0, ...]` under delta). That is sound because zarr uses
/// the value symmetrically — elide on write, restore on read — so a non-zero
/// fill has to survive both an untouched chunk and a partially written one.
#[test]
fn non_zero_fill_value_round_trips() {
    for kind in ["delta", "haar", "lift53"] {
        let fill = 7u16;
        let array = build_array_with_fill(
            Arc::new(MemoryStore::new()),
            "uint16",
            kind,
            FillValue::new(fill.to_ne_bytes().to_vec()),
        );

        // Nothing written yet: every chunk is absent.
        let all: Vec<u16> = array
            .retrieve_array_subset(&array.subset_all())
            .expect("retrieve");
        assert!(all.iter().all(|&v| v == fill), "{kind}: absent chunks");

        // One chunk-aligned region written; the rest stays absent.
        let chunk = [0..4, 0..1, 0..8, 0..8];
        let written = vec![100u16; element_count(&chunk)];
        array.store_array_subset(&chunk, &written).expect("store");
        let region: Vec<u16> = array.retrieve_array_subset(&chunk).expect("retrieve");
        assert_eq!(region, written, "{kind}: the written region");
        let untouched: Vec<u16> = array
            .retrieve_array_subset(&[4..8, 1..2, 0..8, 0..8])
            .expect("retrieve");
        assert!(untouched.iter().all(|&v| v == fill), "{kind}: untouched");

        // A sub-chunk write forces a read-modify-write of a partial chunk:
        // the remainder must still be the fill, not a running sum of it.
        let patch = [4..6, 0..1, 0..2, 0..2];
        array
            .store_array_subset(&patch, vec![250u16; element_count(&patch)])
            .expect("store");
        let patched: Vec<u16> = array.retrieve_array_subset(&patch).expect("retrieve");
        assert!(patched.iter().all(|&v| v == 250), "{kind}: the patch");
        let neighbours: Vec<u16> = array
            .retrieve_array_subset(&[6..8, 0..1, 0..8, 0..8])
            .expect("retrieve");
        assert!(
            neighbours.iter().all(|&v| v == fill),
            "{kind}: the rest of the patched chunk"
        );
    }
}

/// A decoder reads coefficients it did not write. Extreme in-range plane
/// values reach the lifting kernels' arithmetic directly, so every kind must
/// come back with a `CodecError` or garbage samples — never a panic, whatever
/// the profile's `overflow-checks` setting (this test binary has them on).
#[test]
fn hostile_coefficient_planes_do_not_panic_the_decoder() {
    use zarrs::array::codec::api::{ArrayBytes, CodecOptions};

    let coeffs: Vec<i32> = vec![
        i32::MIN,
        i32::MAX,
        i32::MIN + 1,
        i32::MAX - 1,
        0,
        -1,
        i32::MAX,
        i32::MIN,
    ];
    let shape = [std::num::NonZeroU64::new(8).unwrap()];
    let options = CodecOptions::default();
    for kind in ["delta", "haar", "lift53"] {
        let levels = if kind == "delta" { 0 } else { 3 };
        for group in [0, 3] {
            let codec = NdLiftCodec::new_with_configuration(
                &serde_json::from_value(json!({ "version": "0.1", "transforms": [
                    { "axis": "z", "dimension": 0, "kind": kind, "levels": levels,
                      "group": group }
                ] }))
                .unwrap(),
            )
            .expect("codec");
            for dtype_name in ["uint8", "int16", "uint32", "int32"] {
                let (dtype, size) = named_data_type(dtype_name);
                let bytes = ArrayBytes::from(bytemuck::cast_slice::<i32, u8>(&coeffs).to_vec());
                // Ok (the garbage happened to narrow) or Err (it did not) are
                // both fine; reaching this line at all is the assertion.
                let _ = codec.decode(
                    bytes,
                    &shape,
                    &dtype,
                    &FillValue::new(vec![0u8; size]),
                    &options,
                );
            }
        }
    }
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
