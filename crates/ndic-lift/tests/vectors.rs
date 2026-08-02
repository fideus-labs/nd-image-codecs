//! Enforce the committed `nd_lift` `0.1` conformance vectors
//! (`fixtures/nd-lift/vectors.json`): forward output is bit-identical to the
//! frozen expectation and inverse restores the input. The `NumPy` reference
//! implementation asserts against the same file, pinning both to one
//! specification.
#![cfg(feature = "serde")]

use ndic_lift::{AxisTransform, forward, inverse};
use serde_json::Value;

fn i64_list(v: &Value) -> Vec<i64> {
    v.as_array()
        .expect("list")
        .iter()
        .map(|x| x.as_i64().expect("integer"))
        .collect()
}

#[test]
fn conformance_vectors() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/nd-lift/vectors.json"
    );
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(path).expect("run the gen_vectors example first"),
    )
    .expect("valid JSON");
    let cases = doc["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let shape: Vec<usize> = case["shape"]
            .as_array()
            .expect("shape")
            .iter()
            .map(|d| usize::try_from(d.as_u64().expect("dim")).expect("dim"))
            .collect();
        let steps: Vec<AxisTransform> =
            serde_json::from_value(case["configuration"]["transforms"].clone())
                .expect("transforms parse");
        let input = i64_list(&case["input"]);
        let expected = i64_list(&case["expected"]);
        match case["plane"].as_str().expect("plane") {
            "i32" => {
                let original: Vec<i32> = input
                    .iter()
                    .map(|&v| i32::try_from(v).expect("fits i32"))
                    .collect();
                let mut chunk = original.clone();
                forward(&mut chunk, &shape, &steps).expect("forward");
                let widened: Vec<i64> = chunk.iter().copied().map(i64::from).collect();
                assert_eq!(widened, expected, "forward mismatch in case {name}");
                inverse(&mut chunk, &shape, &steps).expect("inverse");
                assert_eq!(chunk, original, "round-trip mismatch in case {name}");
            }
            "i64" => {
                let mut chunk = input.clone();
                forward(&mut chunk, &shape, &steps).expect("forward");
                assert_eq!(chunk, expected, "forward mismatch in case {name}");
                inverse(&mut chunk, &shape, &steps).expect("inverse");
                assert_eq!(chunk, input, "round-trip mismatch in case {name}");
            }
            plane => panic!("unknown plane {plane:?} in case {name}"),
        }
    }
}
