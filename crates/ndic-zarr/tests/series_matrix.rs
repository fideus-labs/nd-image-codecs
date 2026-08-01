//! Shared fixture-matrix test for the `codec_series` builder.
//!
//! Runs every case in `fixtures/codec-series/matrix.json` — the cross-language
//! fixture matrix shared with the Python and TypeScript builders — and asserts
//! the produced pipeline JSON matches the committed expected output (or that
//! the case errors when marked `error`).

use ndic_lift::LiftKind;
use ndic_zarr::series::{Axis, Decorrelate, DeltaBackend, Family, SeriesSpec, codec_series};
use serde_json::Value;

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/codec-series/matrix.json"
    );
    let text = std::fs::read_to_string(path)
        .expect("fixture matrix must exist; see scripts/gen-series-fixtures.py");
    serde_json::from_str(&text).expect("fixture matrix must be valid JSON")
}

fn usize_list(v: &Value) -> Vec<usize> {
    v.as_array()
        .expect("index list")
        .iter()
        .map(|d| usize::try_from(d.as_u64().expect("index")).expect("index fits usize"))
        .collect()
}

/// Build a [`SeriesSpec`] from one fixture case object.
fn spec_from_case(case: &Value) -> SeriesSpec {
    let axes: Vec<Axis> = case["axes"]
        .as_array()
        .expect("axes")
        .iter()
        .enumerate()
        .map(|(i, n)| Axis::new(i, n.as_str().expect("axis name")))
        .collect();
    let chunk_shape: Vec<u64> = case["chunk_shape"]
        .as_array()
        .expect("chunk_shape")
        .iter()
        .map(|c| c.as_u64().expect("chunk size"))
        .collect();
    let family = match case["family"].as_str().expect("family") {
        "nd-delta" => Family::NdDelta,
        "nd-lift-ht" => Family::NdLiftHt,
        "nd-zfp" => Family::NdZfp,
        other => panic!("unknown family {other:?}"),
    };
    let mut spec = SeriesSpec::new(
        axes,
        chunk_shape,
        case["dtype"].as_str().expect("dtype"),
        family,
    );
    let Some(options) = case.get("options").and_then(Value::as_object) else {
        return spec;
    };
    if let Some(exact) = options.get("decorrelate") {
        spec.decorrelate = Decorrelate::Exact(usize_list(exact));
    } else if options.contains_key("add_decorrelate") || options.contains_key("remove_decorrelate")
    {
        spec.decorrelate = Decorrelate::Adjust {
            add: options
                .get("add_decorrelate")
                .map(usize_list)
                .unwrap_or_default(),
            remove: options
                .get("remove_decorrelate")
                .map(usize_list)
                .unwrap_or_default(),
        };
    }
    if let Some(lift) = options.get("lift") {
        spec.lift = match lift.as_str().expect("lift kind") {
            "delta" => LiftKind::Delta,
            "haar" => LiftKind::Haar,
            "lift53" => LiftKind::Lift53,
            other => panic!("unknown lift kind {other:?}"),
        };
    }
    if let Some(levels) = options.get("xy_levels") {
        spec.xy_levels =
            u8::try_from(levels.as_u64().expect("xy_levels")).expect("xy_levels fits u8");
    }
    if let Some(reversible) = options.get("reversible") {
        spec.reversible = reversible.as_bool().expect("reversible");
    }
    if let Some(backend) = options.get("delta_backend") {
        spec.delta_backend = match backend.as_str().expect("delta_backend") {
            "zstd" => DeltaBackend::Zstd,
            "lz4" => DeltaBackend::Lz4,
            other => panic!("unknown delta backend {other:?}"),
        };
    }
    if let Some(rate) = options.get("zfp_rate") {
        spec.zfp_rate = Some(rate.as_f64().expect("zfp_rate"));
    }
    spec
}

#[test]
fn matrix_matches_committed_expectations() {
    let doc = fixture();
    let cases = doc["cases"].as_array().expect("cases");
    assert!(cases.len() > 100, "fixture matrix looks truncated");
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let spec = spec_from_case(case);
        let result = codec_series(&spec);
        if case.get("error").and_then(Value::as_bool) == Some(true) {
            assert!(result.is_err(), "{name}: expected an error, got {result:?}");
            continue;
        }
        let codecs = result.unwrap_or_else(|e| panic!("{name}: builder failed: {e}"));
        assert_eq!(
            Value::Array(codecs.clone()),
            case["expected"],
            "{name}: pipeline diverged from the committed fixture"
        );
    }
}
