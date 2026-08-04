// Shared fixture-data generator for the checksum test and the
// `gen_fixtures` example (which `include!`s this file, so no `//!` docs).
//
// Deterministic forever: the committed checksums in
// `fixtures/zfp/checksums.json` pin the streams these bytes produce.
// Change nothing here without regenerating the fixtures deliberately.

/// The matrix's fixture field: little-endian sample bytes for `n` elements
/// of `dtype` — a smooth ramp with small deterministic xorshift noise.
#[allow(
    dead_code,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)] // value construction is range-checked by the modulus arithmetic
pub fn ramp_chunk(n: usize, dtype: &str) -> Vec<u8> {
    let mut noise: u32 = 0x9e37_79b9;
    (0..n)
        .flat_map(|i| {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let v = (i as i64 % 97) * 3 - 140 + i64::from(noise % 5);
            match dtype {
                "uint8" => vec![v.rem_euclid(256) as u8],
                "int8" => ((v % 127) as i8).to_le_bytes().to_vec(),
                "uint16" => (v.rem_euclid(65536) as u16).to_le_bytes().to_vec(),
                "int16" => ((v % 32000) as i16).to_le_bytes().to_vec(),
                "int32" => ((v * 100_001) as i32).to_le_bytes().to_vec(),
                "int64" => (v * 100_000_007).to_le_bytes().to_vec(),
                "float32" => ((v as f32) / 3.0).to_le_bytes().to_vec(),
                "float64" => ((v as f64) / 3.0).to_le_bytes().to_vec(),
                other => unreachable!("no fixture path for dtype {other}"),
            }
        })
        .collect()
}

/// FNV-1a 64-bit checksum (dependency-free, stable by definition).
#[allow(dead_code)]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// The pinned-chunk fixture's samples: the `(i*7 % 4096)/3` float ramp of
/// `fixtures/zfp/tiny-chunk-4x8x8-rate8.zfp` (shared with the Python
/// byte-identity test).
#[allow(dead_code, clippy::cast_precision_loss)]
pub fn tiny_chunk_f32() -> Vec<u8> {
    (0..4usize * 8 * 8)
        .flat_map(|i| (((i * 7) % 4096) as f32 / 3.0).to_le_bytes())
        .collect()
}

/// The checksum matrix: `(shape, dtype, configuration)` cases, exactly the
/// order committed in `fixtures/zfp/checksums.json`.
#[allow(dead_code)]
pub fn matrix_cases() -> Vec<(Vec<usize>, &'static str, ndic_zfp::NdZfpConfig)> {
    let shapes: [&[usize]; 4] = [&[17], &[9, 10], &[5, 6, 7], &[3, 4, 5, 6]];
    let dtypes = [
        "uint8", "int8", "uint16", "int16", "int32", "int64", "float32", "float64",
    ];
    let mut cases = Vec::new();
    for shape in shapes {
        #[allow(clippy::cast_possible_truncation)] // ranks are 1..=4
        let dims = Some(shape.len() as u8);
        for dtype in dtypes {
            let mut modes = vec![
                ndic_zfp::NdZfpConfig {
                    mode: "reversible".into(),
                    dims,
                    ..Default::default()
                },
                ndic_zfp::NdZfpConfig {
                    mode: "fixed_rate".into(),
                    rate: Some(8.0),
                    dims,
                    ..Default::default()
                },
                ndic_zfp::NdZfpConfig {
                    mode: "fixed_precision".into(),
                    precision: Some(16),
                    dims,
                    ..Default::default()
                },
            ];
            if dtype.starts_with("float") {
                // Fixed accuracy is only meaningful for float data.
                modes.push(ndic_zfp::NdZfpConfig {
                    mode: "fixed_accuracy".into(),
                    tolerance: Some(0.01),
                    dims,
                    ..Default::default()
                });
            }
            for config in modes {
                cases.push((shape.to_vec(), dtype, config));
            }
        }
    }
    cases
}
