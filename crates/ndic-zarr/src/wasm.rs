//! WASM (`wasm-bindgen`) exports for the TypeScript / numcodecs.js binding.
//!
//! Thin wrappers over the feature-free chunk cores in [`crate::htj2k`] and
//! [`ndic_zfp`] — the same functions the `zarrs` codecs and the Python
//! extension call, so the browser produces byte-identical chunks. Build
//! with `npm run build:wasm` in `bindings/typescript` (wasm-pack,
//! `--target web`).

use wasm_bindgen::prelude::*;

use ndic_core::SampleType;
use ndic_lift::NdLiftConfig;
use ndic_zfp::{NdZfpConfig, ZfpDtype};

use crate::htj2k::{Htj2kConfig, decode_chunk, encode_chunk};
use crate::lift::LiftDtype;

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

fn lift_dtype(dtype: &str) -> Result<LiftDtype, JsError> {
    LiftDtype::from_zarr_name(dtype)
        .ok_or_else(|| JsError::new(&format!("nd_lift has no integer path for dtype {dtype:?}")))
}

fn lift_config(config_json: &str) -> Result<NdLiftConfig, JsError> {
    serde_json::from_str(config_json)
        .map_err(|e| JsError::new(&format!("nd_lift configuration: {e}")))
}

/// Encodes a chunk (little-endian elements, C order) into its widened,
/// decorrelated `nd_lift` coefficient plane (little-endian `int32` or
/// `int64`). `config_json` is the Zarr v3 `configuration` object.
#[wasm_bindgen]
pub fn nd_lift_encode(
    chunk: &[u8],
    shape: &[u32],
    dtype: &str,
    config_json: &str,
) -> Result<Vec<u8>, JsError> {
    crate::lift::forward_chunk(
        chunk,
        &shape_usize(shape),
        lift_dtype(dtype)?,
        &lift_config(config_json)?,
    )
    .map_err(|e| JsError::new(&e.to_string()))
}

/// Decodes an `nd_lift` coefficient plane back to little-endian `dtype`
/// elements in C order.
#[wasm_bindgen]
pub fn nd_lift_decode(
    chunk: &[u8],
    shape: &[u32],
    dtype: &str,
    config_json: &str,
) -> Result<Vec<u8>, JsError> {
    crate::lift::inverse_chunk(
        chunk,
        &shape_usize(shape),
        lift_dtype(dtype)?,
        &lift_config(config_json)?,
    )
    .map_err(|e| JsError::new(&e.to_string()))
}

fn zfp_dtype(dtype: &str) -> Result<ZfpDtype, JsError> {
    ZfpDtype::from_zarr_name(dtype)
        .ok_or_else(|| JsError::new(&format!("nd_zfp has no path for dtype {dtype:?}")))
}

fn zfp_config(config_json: &str) -> Result<NdZfpConfig, JsError> {
    serde_json::from_str(config_json)
        .map_err(|e| JsError::new(&format!("nd_zfp configuration: {e}")))
}

/// Encodes a chunk (little-endian elements, C order) into an `nd_zfp` ZFP
/// stream. `config_json` is the Zarr v3 `configuration` object (`{}` for
/// the defaults).
#[wasm_bindgen]
pub fn nd_zfp_encode(
    chunk: &[u8],
    shape: &[u32],
    dtype: &str,
    config_json: &str,
) -> Result<Vec<u8>, JsError> {
    ndic_zfp::encode_chunk(
        chunk,
        &shape_usize(shape),
        zfp_dtype(dtype)?,
        &zfp_config(config_json)?,
    )
    .map_err(|e| JsError::new(&e.to_string()))
}

/// Decodes an `nd_zfp` chunk back to little-endian elements in C order.
/// The stream's header must match the configuration in `config_json`.
#[wasm_bindgen]
pub fn nd_zfp_decode(
    chunk: &[u8],
    shape: &[u32],
    dtype: &str,
    config_json: &str,
) -> Result<Vec<u8>, JsError> {
    ndic_zfp::decode_chunk(
        chunk,
        &shape_usize(shape),
        zfp_dtype(dtype)?,
        &zfp_config(config_json)?,
    )
    .map_err(|e| JsError::new(&e.to_string()))
}
