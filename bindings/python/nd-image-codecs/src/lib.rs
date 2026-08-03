//! `_nd_image_codecs` — the native extension module behind the `nd-image-codecs`
//! Python package.
//!
//! Built with [maturin](https://www.maturin.rs/) against the stable
//! `abi3-py311` ABI. The Python-side codec classes and the pure-Python
//! `codec_series` builder live in `python/nd_image_codecs/`; the `htj2k`
//! zarr codec (`zarr_codec.Htj2kCodec`) calls the chunk functions exported
//! here, which wrap the same feature-free core (`ndic_zarr::htj2k`) as the
//! Rust `zarrs` codec and the WASM binding — every ecosystem produces
//! byte-identical chunks.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use ndic_core::SampleType;
use ndic_zarr::htj2k::{Htj2kConfig, decode_chunk, encode_chunk};

/// Version string reported by `nd_image_codecs.__version__`.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn sample_type(dtype: &str) -> PyResult<SampleType> {
    match dtype {
        "uint8" | "|u1" | "u1" => Ok(SampleType::U8),
        "int8" | "|i1" | "i1" => Ok(SampleType::I8),
        "uint16" | "<u2" | "u2" => Ok(SampleType::U16),
        "int16" | "<i2" | "i2" => Ok(SampleType::I16),
        "uint32" | "<u4" | "u4" => Ok(SampleType::U32),
        "int32" | "<i4" | "i4" => Ok(SampleType::I32),
        other => Err(PyValueError::new_err(format!(
            "htj2k has no integer path for dtype {other:?}"
        ))),
    }
}

/// Encodes a chunk (native-endian elements, C order, trailing dims `(y, x)`)
/// into the `htj2k` container `[header | plane index | codestreams…]`.
#[pyfunction]
#[pyo3(signature = (chunk, shape, dtype, *, xy_levels=5, reversible=true, progression="RPCL", index=true))]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
// `Vec<usize>`: pyo3 extracts owned collections from Python lists.
#[allow(clippy::needless_pass_by_value)]
fn htj2k_encode<'py>(
    py: Python<'py>,
    chunk: &[u8],
    shape: Vec<usize>,
    dtype: &str,
    xy_levels: u8,
    reversible: bool,
    progression: &str,
    index: bool,
) -> PyResult<Bound<'py, PyBytes>> {
    let config = Htj2kConfig {
        xy_levels,
        reversible,
        progression: progression.to_owned(),
        index,
    };
    let out = encode_chunk(chunk, &shape, sample_type(dtype)?, &config)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(PyBytes::new(py, &out))
}

/// Decodes an `htj2k` chunk back to native-endian elements in C order.
#[pyfunction]
// `Vec<usize>`: pyo3 extracts owned collections from Python lists.
#[allow(clippy::needless_pass_by_value)]
fn htj2k_decode<'py>(
    py: Python<'py>,
    chunk: &[u8],
    shape: Vec<usize>,
    dtype: &str,
) -> PyResult<Bound<'py, PyBytes>> {
    let out = decode_chunk(chunk, &shape, sample_type(dtype)?)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(PyBytes::new(py, &out))
}

/// The native module. Registered as `nd_image_codecs._nd_image_codecs`.
#[pymodule]
fn _nd_image_codecs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(htj2k_encode, m)?)?;
    m.add_function(wrap_pyfunction!(htj2k_decode, m)?)?;
    Ok(())
}
