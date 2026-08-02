//! Regenerate `fixtures/nd-lift/vectors.json` — the committed `nd_lift` `0.1`
//! conformance vectors.
//!
//! The expected outputs freeze the transform semantics (predictor, update,
//! rounding, boundary extension, coefficient ordering); the Rust and `NumPy`
//! implementations both assert bit-identical forward outputs against this
//! file. Regenerate **only** on a deliberate, version-bumping semantics
//! change:
//!
//! ```sh
//! cargo run -p ndic-lift --features serde --example gen_vectors
//! ```

use ndic_lift::{AxisTransform, LiftKind, forward};
use serde_json::{Value, json};

fn step(axis: &str, dimension: usize, kind: LiftKind, levels: u8, group: u32) -> AxisTransform {
    AxisTransform {
        axis: axis.into(),
        dimension,
        kind,
        levels,
        group,
    }
}

/// A tiny deterministic value sequence (LCG), also reproduced in the Python
/// test suite.
fn lcg(len: usize, modulus: i64, offset: i64) -> Vec<i64> {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            i64::try_from(state >> 33).expect("31 bits") % modulus + offset
        })
        .collect()
}

fn case(name: &str, shape: &[usize], plane: &str, input: &[i64], steps: &[AxisTransform]) -> Value {
    let expected: Vec<i64> = if plane == "i32" {
        let mut chunk: Vec<i32> = input
            .iter()
            .map(|&v| i32::try_from(v).expect("i32 case values fit i32"))
            .collect();
        forward(&mut chunk, shape, steps).expect("vector case must encode");
        chunk.into_iter().map(i64::from).collect()
    } else {
        let mut chunk = input.to_vec();
        forward(&mut chunk, shape, steps).expect("vector case must encode");
        chunk
    };
    json!({
        "name": name,
        "shape": shape,
        "plane": plane,
        "configuration": {
            "version": "0.1",
            "transforms": steps,
        },
        "input": input,
        "expected": expected,
    })
}

#[allow(clippy::too_many_lines)]
fn main() {
    let ramp8: Vec<i64> = (0..8).collect();
    let dc7 = vec![5i64; 7];
    let cases = vec![
        case(
            "delta-ramp",
            &[7],
            "i32",
            &[3, 4, 5, 6, 7, 8, 9],
            &[step("z", 0, LiftKind::Delta, 0, 0)],
        ),
        case(
            "delta-group-2",
            &[4],
            "i32",
            &[10, 11, 20, 23],
            &[step("z", 0, LiftKind::Delta, 0, 2)],
        ),
        case(
            "haar-dc",
            &[6],
            "i32",
            &[9, 9, 9, 9, 9, 9],
            &[step("z", 0, LiftKind::Haar, 1, 0)],
        ),
        case(
            "haar-signed-pairs",
            &[4],
            "i32",
            &[2, 7, -2, -7],
            &[step("z", 0, LiftKind::Haar, 1, 0)],
        ),
        case(
            "haar-two-levels-odd",
            &[5],
            "i32",
            &[1, 2, 4, 8, 16],
            &[step("z", 0, LiftKind::Haar, 2, 0)],
        ),
        case(
            "lift53-impulse",
            &[4],
            "i32",
            &[0, 4, 0, 0],
            &[step("z", 0, LiftKind::Lift53, 1, 0)],
        ),
        case(
            "lift53-ramp",
            &[8],
            "i32",
            &ramp8,
            &[step("z", 0, LiftKind::Lift53, 1, 0)],
        ),
        case(
            "lift53-dc-two-levels-odd",
            &[7],
            "i32",
            &dc7,
            &[step("z", 0, LiftKind::Lift53, 2, 0)],
        ),
        case(
            "lift53-group-3-odd-tail",
            &[5],
            "i32",
            &[100, 90, 70, 40, 0],
            &[step("z", 0, LiftKind::Lift53, 2, 3)],
        ),
        case(
            "volume-lift53-z-delta-t",
            &[3, 2, 4],
            "i32",
            &lcg(24, 1 << 16, 0),
            &[
                step("z", 0, LiftKind::Lift53, 2, 0),
                step("t", 2, LiftKind::Delta, 0, 2),
            ],
        ),
        case(
            "volume-haar-then-lift53-same-axis",
            &[6, 3],
            "i32",
            &lcg(18, 1 << 12, -(1 << 11)),
            &[
                step("z", 0, LiftKind::Haar, 1, 0),
                step("z", 0, LiftKind::Lift53, 1, 0),
            ],
        ),
        case(
            "singleton-axis-noop",
            &[1, 4],
            "i32",
            &[7, -8, 9, -10],
            &[
                step("z", 0, LiftKind::Lift53, 2, 0),
                step("x", 1, LiftKind::Delta, 0, 0),
            ],
        ),
        case(
            "i64-plane-wide-values",
            &[6],
            "i64",
            &lcg(6, 1 << 30, -(1 << 29))
                .iter()
                .map(|v| v * (1 << 16))
                .collect::<Vec<_>>(),
            &[step("z", 0, LiftKind::Lift53, 2, 0)],
        ),
    ];
    let doc = json!({
        "description": "nd_lift 0.1 conformance vectors — frozen forward outputs; \
                        regenerate only on a version-bumping semantics change \
                        (cargo run -p ndic-lift --features serde --example gen_vectors)",
        "version": "0.1",
        "cases": cases,
    });
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/nd-lift/vectors.json"
    );
    std::fs::create_dir_all(std::path::Path::new(path).parent().expect("parent"))
        .expect("create fixtures/nd-lift");
    std::fs::write(
        path,
        serde_json::to_string_pretty(&doc).expect("json") + "\n",
    )
    .expect("write vectors.json");
    println!("wrote {path}");
}
