//! `_nd_image_codecs` — the native extension module behind the `nd-image-codecs`
//! Python package.
//!
//! Built with [maturin](https://www.maturin.rs/) against the stable
//! `abi3-py311` ABI. The Python-side codec classes (`NdLift`, `Htj2k`,
//! `NdZfp`) and the pure-Python `codec_series` builder live in
//! `python/nd_image_codecs/__init__.py`; once the codecs are implemented they
//! will call the `encode`/`decode` functions exported here (roadmap Phases
//! 2–5).

use pyo3::prelude::*;

/// Version string reported by `nd_image_codecs.__version__`.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The native module. Registered as `nd_image_codecs._nd_image_codecs`.
#[pymodule]
fn _nd_image_codecs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    Ok(())
}
