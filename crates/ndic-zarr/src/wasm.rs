//! WASM (`wasm-bindgen`) exports for the TypeScript / numcodecs.js binding.
//!
//! Thin wrappers over the feature-free chunk core in [`crate::htj2k`] — the
//! same functions the `zarrs` codec and the Python extension call, so the
//! browser produces byte-identical chunks. Build with
//! `npm run build:wasm` in `bindings/typescript` (wasm-pack, `--target
//! web`).

use wasm_bindgen::prelude::*;

use ndic_core::SampleType;

use crate::htj2k::{Htj2kConfig, decode_chunk, encode_chunk};

fn sample_type(dtype: &str) -> Result<SampleType, JsError> {
    match dtype {
        "uint8" | "|u1" | "u1" => Ok(SampleType::U8),
        "int8" | "|i1" | "i1" => Ok(SampleType::I8),
        "uint16" | "<u2" | "u2" => Ok(SampleType::U16),
        "int16" | "<i2" | "i2" => Ok(SampleType::I16),
        "uint32" | "<u4" | "u4" => Ok(SampleType::U32),
        "int32" | "<i4" | "i4" => Ok(SampleType::I32),
        other => Err(JsError::new(&format!(
            "htj2k has no integer path for dtype {other:?}"
        ))),
    }
}

fn shape_usize(shape: &[u32]) -> Vec<usize> {
    shape.iter().map(|&d| d as usize).collect()
}

/// Encodes a chunk (little-endian elements, C order, trailing dims
/// `(y, x)`) into the `htj2k` container. `config_json` is the Zarr v3
/// `configuration` object (`{}` for the defaults).
#[wasm_bindgen]
pub fn htj2k_encode(
    chunk: &[u8],
    shape: &[u32],
    dtype: &str,
    config_json: &str,
) -> Result<Vec<u8>, JsError> {
    let config: Htj2kConfig = serde_json::from_str(config_json)
        .map_err(|e| JsError::new(&format!("htj2k configuration: {e}")))?;
    encode_chunk(chunk, &shape_usize(shape), sample_type(dtype)?, &config)
        .map_err(|e| JsError::new(&e.to_string()))
}

/// Decodes an `htj2k` chunk back to little-endian elements in C order.
#[wasm_bindgen]
pub fn htj2k_decode(chunk: &[u8], shape: &[u32], dtype: &str) -> Result<Vec<u8>, JsError> {
    decode_chunk(chunk, &shape_usize(shape), sample_type(dtype)?)
        .map_err(|e| JsError::new(&e.to_string()))
}
