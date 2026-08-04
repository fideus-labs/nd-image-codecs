//! Codec-series builder: axis metadata → a complete Zarr v3 codec pipeline.
//!
//! Given each array dimension's **index** and **string axis identifier**
//! (`"t"`, `"c"`, `"z"`, `"y"`, `"x"`, or any custom NGFF axis name) plus the
//! chunk shape, [`codec_series`] produces the codec list for one of the three
//! nd-image-codecs families:
//!
//! - **nd-delta** — `transpose → numcodecs.delta → bytes → blosc(bitshuffle, zstd|lz4)`
//! - **nd-lift-ht** — `transpose → nd_lift → htj2k`
//! - **nd-zfp** — `transpose → reshape → zfp`
//!
//! Defaults (each overridable):
//! - The fastest-moving dimensions are transposed into `(z)yx` order.
//! - `t` is placed immediately before `z` (or `y` when there is no `z`) when
//!   its chunk size — the grouping size — is not 1; otherwise it stays with
//!   the leading, untransformed dimensions.
//! - Decorrelation is applied along `z`, and along `t` when its chunk size is
//!   not 1. [`Decorrelate`] disables these defaults or enables other
//!   dimensions with index lists.
//!
//! The same algorithm is mirrored by `nd_image_codecs.codec_series` in the
//! Python package and `codecSeries` in the TypeScript package; Phase 1 CI
//! cross-checks all three. See `docs/architecture/codec-series.md`.

use ndic_lift::LiftKind;
use serde_json::{Value, json};

/// One array dimension: its index in the array shape and its axis name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Axis {
    /// Dimension index into the array/chunk shape.
    pub index: usize,
    /// Axis identifier, e.g. `"t"`, `"c"`, `"z"`, `"y"`, `"x"`.
    pub name: String,
}

impl Axis {
    /// Convenience constructor.
    pub fn new(index: usize, name: &str) -> Self {
        Self {
            index,
            name: name.into(),
        }
    }
}

/// Which nd-image-codecs family the series targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// `transpose → delta → bitshuffle → zstd/lz4` from existing codecs.
    NdDelta,
    /// `transpose → nd_lift → htj2k` coefficient planes.
    NdLiftHt,
    /// `transpose → reshape → zfp` blocks with a brick index.
    NdZfp,
}

/// Decorrelation-axis selection, as dimension-index lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Decorrelate {
    /// `z`, plus `t` when its chunk size is not 1.
    #[default]
    Defaults,
    /// Exactly these dimension indices (replaces the defaults).
    Exact(Vec<usize>),
    /// The defaults, plus `add`, minus `remove`.
    Adjust {
        /// Dimension indices to additionally decorrelate (e.g. a correlated
        /// channel axis).
        add: Vec<usize>,
        /// Dimension indices to exclude from the defaults.
        remove: Vec<usize>,
    },
}

/// Backend compressor for the nd-delta family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeltaBackend {
    /// Blosc-Zstd (best ratio).
    #[default]
    Zstd,
    /// Blosc-LZ4 (fastest).
    Lz4,
}

/// Everything [`codec_series`] needs.
#[derive(Debug, Clone)]
pub struct SeriesSpec {
    /// One entry per array dimension, any order; indices must cover
    /// `0..ndim` exactly once.
    pub axes: Vec<Axis>,
    /// Chunk shape, indexed by dimension index.
    pub chunk_shape: Vec<u64>,
    /// Zarr v3 data type name, e.g. `"uint16"`.
    pub dtype: String,
    /// Which codec family to build.
    pub family: Family,
    /// Decorrelation-axis selection.
    pub decorrelate: Decorrelate,
    /// Transform kind for decorrelation axes (nd-lift-ht).
    pub lift: LiftKind,
    /// In-plane resolution levels for the htj2k plane codec.
    pub xy_levels: u8,
    /// Reversible (lossless) coding.
    pub reversible: bool,
    /// Blosc backend for nd-delta.
    pub delta_backend: DeltaBackend,
    /// ZFP bits-per-value for nd-zfp; `None` = reversible mode.
    pub zfp_rate: Option<f64>,
}

impl SeriesSpec {
    /// A spec with family defaults: 5/3 lifting, 5 xy levels, reversible.
    pub fn new(axes: Vec<Axis>, chunk_shape: Vec<u64>, dtype: &str, family: Family) -> Self {
        Self {
            axes,
            chunk_shape,
            dtype: dtype.into(),
            family,
            decorrelate: Decorrelate::Defaults,
            lift: LiftKind::Lift53,
            xy_levels: 5,
            reversible: true,
            delta_backend: DeltaBackend::default(),
            zfp_rate: None,
        }
    }
}

/// Errors from [`codec_series`].
#[derive(Debug, thiserror::Error)]
pub enum SeriesError {
    /// Axis indices are not a permutation of `0..ndim`, or shape mismatch.
    #[error("invalid axes: {0}")]
    InvalidAxes(String),
    /// The data type is not recognized / not supported by the family.
    #[error("unsupported dtype {dtype:?} for {family:?}")]
    UnsupportedDtype {
        /// Requested Zarr data type name.
        dtype: String,
        /// Requested family.
        family: Family,
    },
    /// A decorrelation override references a bad dimension.
    #[error("invalid decorrelation dimension {0}: {1}")]
    InvalidDecorrelation(usize, String),
    /// nd-zfp supports at most 4 non-singleton chunk dimensions.
    #[error("nd-zfp needs ≤4 non-singleton chunk dimensions, got {0}; reduce chunking")]
    TooManyZfpDims(usize),
}

/// (zarr dtype, numpy little-endian code, item size in bytes)
const DTYPES: &[(&str, &str, u32)] = &[
    ("uint8", "|u1", 1),
    ("int8", "|i1", 1),
    ("uint16", "<u2", 2),
    ("int16", "<i2", 2),
    ("uint32", "<u4", 4),
    ("int32", "<i4", 4),
    ("uint64", "<u8", 8),
    ("int64", "<i8", 8),
    ("float32", "<f4", 4),
    ("float64", "<f8", 8),
];

fn dtype_info(dtype: &str) -> Option<(&'static str, u32)> {
    DTYPES
        .iter()
        .find(|(z, ..)| *z == dtype)
        .map(|&(_, np, size)| (np, size))
}

/// Contiguous input-dimension groups collapsing singletons for `reshape`:
/// one group per non-singleton dimension, each absorbing the singleton
/// dimensions before it; trailing singletons merge into the final group,
/// and an all-singleton shape becomes one group (a 1-element 1D field).
/// Mirrored exactly by the Python and TypeScript builders.
fn reshape_groups(shape: &[u64]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for (i, &extent) in shape.iter().enumerate() {
        current.push(i);
        if extent > 1 {
            groups.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        if let Some(last) = groups.last_mut() {
            last.append(&mut current);
        } else {
            groups.push(current);
        }
    }
    groups
}

/// Build the codec pipeline for `spec`. Returns Zarr v3 codec metadata
/// objects, in application order, ready to serialize into array metadata.
///
/// # Errors
/// See [`SeriesError`].
#[allow(clippy::too_many_lines)]
pub fn codec_series(spec: &SeriesSpec) -> Result<Vec<Value>, SeriesError> {
    let ndim = spec.axes.len();
    // -- validate axes ---------------------------------------------------
    if spec.chunk_shape.len() != ndim {
        return Err(SeriesError::InvalidAxes(format!(
            "{ndim} axes but chunk shape has {} entries",
            spec.chunk_shape.len()
        )));
    }
    let mut seen = vec![false; ndim];
    for ax in &spec.axes {
        if ax.index >= ndim || seen[ax.index] {
            return Err(SeriesError::InvalidAxes(format!(
                "axis indices must cover 0..{ndim} exactly once (bad index {})",
                ax.index
            )));
        }
        seen[ax.index] = true;
    }
    let find = |name: &str| spec.axes.iter().find(|a| a.name == name).map(|a| a.index);
    let (Some(x), Some(y)) = (find("x"), find("y")) else {
        return Err(SeriesError::InvalidAxes(
            "an 'x' and a 'y' axis are required".into(),
        ));
    };
    let z = find("z");
    let t = find("t");
    let chunk = |d: usize| spec.chunk_shape[d];
    let (np_dtype, itemsize) =
        dtype_info(&spec.dtype).ok_or_else(|| SeriesError::UnsupportedDtype {
            dtype: spec.dtype.clone(),
            family: spec.family,
        })?;
    if spec.family == Family::NdLiftHt && np_dtype.contains('f') {
        // Float planes need the lossy path; reversible HT coding is integer.
        if spec.reversible {
            return Err(SeriesError::UnsupportedDtype {
                dtype: spec.dtype.clone(),
                family: spec.family,
            });
        }
    }

    // -- decorrelation set (original dimension indices) -------------------
    let mut decorr: Vec<usize> = Vec::new();
    let defaults: Vec<usize> = [z, t]
        .into_iter()
        .flatten()
        .filter(|&d| chunk(d) > 1)
        .collect();
    match &spec.decorrelate {
        Decorrelate::Defaults => decorr = defaults,
        Decorrelate::Exact(list) => decorr.extend(list),
        Decorrelate::Adjust { add, remove } => {
            decorr = defaults;
            for &d in add {
                if !decorr.contains(&d) {
                    decorr.push(d);
                }
            }
            decorr.retain(|d| !remove.contains(d));
        }
    }
    for &d in &decorr {
        if d >= ndim {
            return Err(SeriesError::InvalidDecorrelation(d, "out of range".into()));
        }
        if d == x || d == y {
            return Err(SeriesError::InvalidDecorrelation(
                d,
                "the primary spatial axes (x, y) are decorrelated by the 2D codec itself".into(),
            ));
        }
    }
    decorr.sort_unstable();
    decorr.dedup();

    // -- target dimension order -------------------------------------------
    // [ leading (original order) ... , extra decorrelated ... , t?, z?, y, x ]
    let t_grouped = t.is_some_and(|t| chunk(t) > 1 && decorr.contains(&t));
    let mut trailing: Vec<usize> = Vec::new();
    if let Some(t) = t
        && t_grouped
    {
        trailing.push(t);
    }
    if let Some(z) = z {
        trailing.push(z);
    }
    trailing.push(y);
    trailing.push(x);
    let extra: Vec<usize> = decorr
        .iter()
        .copied()
        .filter(|d| !trailing.contains(d))
        .collect();
    let mut order: Vec<usize> = (0..ndim)
        .filter(|d| !trailing.contains(d) && !extra.contains(d))
        .collect();
    order.extend(&extra);
    order.extend(&trailing);

    // nd-delta: numcodecs.delta differences the flattened chunk, so the delta
    // axis must be the fastest-moving one — move it last.
    if spec.family == Family::NdDelta
        && let Some(a) = [z, t].into_iter().flatten().find(|d| decorr.contains(d))
    {
        order.retain(|&d| d != a);
        order.push(a);
    }

    let mut codecs: Vec<Value> = Vec::new();
    if order.iter().copied().ne(0..ndim) {
        codecs.push(json!({
            "name": "transpose",
            "configuration": { "order": order }
        }));
    }

    // -- family tail -------------------------------------------------------
    let pos_of = |d: usize| order.iter().position(|&o| o == d).unwrap_or(d);
    match spec.family {
        Family::NdDelta => {
            codecs.push(json!({
                "name": "numcodecs.delta",
                "configuration": { "dtype": np_dtype }
            }));
            codecs.push(json!({
                "name": "bytes",
                "configuration": { "endian": "little" }
            }));
            let cname = match spec.delta_backend {
                DeltaBackend::Zstd => "zstd",
                DeltaBackend::Lz4 => "lz4",
            };
            codecs.push(json!({
                "name": "blosc",
                "configuration": {
                    "cname": cname,
                    "clevel": 5,
                    "shuffle": "bitshuffle",
                    "typesize": itemsize,
                    "blocksize": 0
                }
            }));
        }
        Family::NdLiftHt => {
            let transforms: Vec<Value> = decorr
                .iter()
                .map(|&d| {
                    let name = &spec.axes.iter().find(|a| a.index == d).unwrap().name;
                    json!({
                        "axis": name,
                        "dimension": pos_of(d),
                        "kind": spec.lift.as_str(),
                        "levels": if spec.lift == LiftKind::Delta { 0 } else { 2 },
                        "group": 0
                    })
                })
                .collect();
            if !transforms.is_empty() {
                codecs.push(json!({
                    "name": "nd_lift",
                    "configuration": { "version": "0.1", "transforms": transforms }
                }));
            }
            codecs.push(json!({
                "name": "htj2k",
                "configuration": {
                    "xy_levels": spec.xy_levels,
                    "reversible": spec.reversible,
                    "progression": "RPCL",
                    "index": true
                }
            }));
        }
        Family::NdZfp => {
            let nonsingleton = spec.chunk_shape.iter().filter(|&&c| c > 1).count();
            if nonsingleton > 4 {
                return Err(SeriesError::TooManyZfpDims(nonsingleton));
            }
            // Collapse singleton dimensions with a `reshape` codec so the
            // chunk reaching `zfp` is a direct 1–4D field, as the registered
            // codec specifies. Groups are contiguous post-transpose
            // dimension indices, one per output dimension; trailing
            // singletons merge into the final group.
            let transposed: Vec<u64> = order.iter().map(|&d| spec.chunk_shape[d]).collect();
            if transposed.contains(&1) {
                codecs.push(json!({
                    "name": "reshape",
                    "configuration": { "shape": reshape_groups(&transposed) }
                }));
            }
            let mode = spec.zfp_rate.map_or_else(
                || json!({ "mode": "reversible" }),
                |rate| json!({ "mode": "fixed_rate", "rate": rate }),
            );
            codecs.push(json!({ "name": "zfp", "configuration": mode }));
        }
    }
    Ok(codecs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tczyx() -> Vec<Axis> {
        ["t", "c", "z", "y", "x"]
            .iter()
            .enumerate()
            .map(|(i, n)| Axis::new(i, n))
            .collect()
    }

    #[test]
    fn lift_ht_tczyx_groups_t_and_z() {
        let spec = SeriesSpec::new(
            tczyx(),
            vec![8, 1, 32, 256, 256],
            "uint16",
            Family::NdLiftHt,
        );
        let codecs = codec_series(&spec).unwrap();
        // c leads; t (grouped) before z; then y, x.
        assert_eq!(codecs[0]["name"], "transpose");
        assert_eq!(codecs[0]["configuration"]["order"], json!([1, 0, 2, 3, 4]));
        assert_eq!(codecs[1]["name"], "nd_lift");
        let tf = codecs[1]["configuration"]["transforms"].as_array().unwrap();
        assert_eq!(tf.len(), 2);
        assert_eq!(
            (tf[0]["axis"].as_str(), tf[0]["dimension"].as_u64()),
            (Some("t"), Some(1))
        );
        assert_eq!(
            (tf[1]["axis"].as_str(), tf[1]["dimension"].as_u64()),
            (Some("z"), Some(2))
        );
        assert_eq!(codecs[2]["name"], "htj2k");
    }

    #[test]
    fn t_chunk_of_one_stays_leading_and_untransformed() {
        let spec = SeriesSpec::new(
            tczyx(),
            vec![1, 1, 32, 256, 256],
            "uint16",
            Family::NdLiftHt,
        );
        let codecs = codec_series(&spec).unwrap();
        // t chunk == 1 → no grouping: order is identity, no transpose.
        assert_eq!(codecs[0]["name"], "nd_lift");
        let tf = codecs[0]["configuration"]["transforms"].as_array().unwrap();
        assert_eq!(tf.len(), 1);
        assert_eq!(tf[0]["axis"].as_str(), Some("z"));
    }

    #[test]
    fn xyz_input_is_transposed_to_zyx() {
        let axes = vec![Axis::new(0, "x"), Axis::new(1, "y"), Axis::new(2, "z")];
        let spec = SeriesSpec::new(axes, vec![256, 256, 32], "float32", Family::NdZfp);
        let codecs = codec_series(&spec).unwrap();
        assert_eq!(codecs[0]["configuration"]["order"], json!([2, 1, 0]));
        // No singleton chunk dimensions → no reshape stage.
        assert_eq!(codecs[1]["name"], "zfp");
        assert_eq!(codecs[1]["configuration"], json!({ "mode": "reversible" }));
    }

    #[test]
    fn delta_axis_moves_fastest() {
        let spec = SeriesSpec::new(tczyx(), vec![8, 1, 32, 256, 256], "uint16", Family::NdDelta);
        let codecs = codec_series(&spec).unwrap();
        // z is the delta axis → transposed to the last (fastest) position.
        assert_eq!(codecs[0]["configuration"]["order"], json!([1, 0, 3, 4, 2]));
        assert_eq!(codecs[1]["name"], "numcodecs.delta");
        assert_eq!(codecs[1]["configuration"]["dtype"], json!("<u2"));
        assert_eq!(codecs[2]["name"], "bytes");
        assert_eq!(codecs[3]["name"], "blosc");
        assert_eq!(codecs[3]["configuration"]["cname"], json!("zstd"));
        assert_eq!(codecs[3]["configuration"]["typesize"], json!(2));
    }

    #[test]
    fn exact_override_decorrelates_channels_only() {
        let spec = SeriesSpec {
            decorrelate: Decorrelate::Exact(vec![1]),
            ..SeriesSpec::new(
                tczyx(),
                vec![1, 4, 32, 256, 256],
                "uint16",
                Family::NdLiftHt,
            )
        };
        let codecs = codec_series(&spec).unwrap();
        // c decorrelated → placed just before the trailing zyx block; t leads.
        assert_eq!(codecs.len(), 2, "identity order → no transpose: {codecs:?}");
        let tf = codecs[0]["configuration"]["transforms"].as_array().unwrap();
        assert_eq!(tf.len(), 1);
        assert_eq!(
            (tf[0]["axis"].as_str(), tf[0]["dimension"].as_u64()),
            (Some("c"), Some(1))
        );
    }

    #[test]
    fn remove_default_disables_z() {
        let spec = SeriesSpec {
            decorrelate: Decorrelate::Adjust {
                add: vec![],
                remove: vec![2],
            },
            ..SeriesSpec::new(
                tczyx(),
                vec![1, 1, 32, 256, 256],
                "uint16",
                Family::NdLiftHt,
            )
        };
        let codecs = codec_series(&spec).unwrap();
        assert_eq!(
            codecs.len(),
            1,
            "no transforms and identity order → htj2k only"
        );
        assert_eq!(codecs[0]["name"], "htj2k");
    }

    #[test]
    fn primary_spatial_axes_are_rejected() {
        let spec = SeriesSpec {
            decorrelate: Decorrelate::Exact(vec![4]),
            ..SeriesSpec::new(
                tczyx(),
                vec![1, 1, 32, 256, 256],
                "uint16",
                Family::NdLiftHt,
            )
        };
        assert!(matches!(
            codec_series(&spec),
            Err(SeriesError::InvalidDecorrelation(4, _))
        ));
    }

    #[test]
    fn zfp_singletons_collapse_via_reshape() {
        let spec = SeriesSpec::new(tczyx(), vec![8, 1, 32, 256, 256], "uint16", Family::NdZfp);
        let codecs = codec_series(&spec).unwrap();
        assert_eq!(codecs[0]["name"], "transpose");
        assert_eq!(codecs[1]["name"], "reshape");
        // Transposed chunk shape is (c=1, t=8, 32, 256, 256): the leading
        // singleton folds into the first group.
        assert_eq!(
            codecs[1]["configuration"]["shape"],
            json!([[0, 1], [2], [3], [4]])
        );
        assert_eq!(codecs[2]["name"], "zfp");
        assert_eq!(codecs[2]["configuration"], json!({ "mode": "reversible" }));
    }

    #[test]
    fn zfp_rejects_more_than_four_dims() {
        let spec = SeriesSpec::new(tczyx(), vec![8, 4, 32, 256, 256], "float32", Family::NdZfp);
        assert!(matches!(
            codec_series(&spec),
            Err(SeriesError::TooManyZfpDims(5))
        ));
    }
}
