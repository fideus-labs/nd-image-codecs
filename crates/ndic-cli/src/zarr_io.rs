//! `ndic zarr` — Zarr v3 store I/O (feature `zarr`).
//!
//! Writes an array to a filesystem store with a codec-series pipeline built
//! by [`ndic_zarr::series::codec_series`], and reads one back to raw
//! little-endian element bytes. This is the `zarrs` corner of the Phase 6
//! cross-ecosystem validation matrix: the same case spec drives the
//! zarr-python and zarrita.js writers/readers, and the orchestrator
//! (`scripts/ci/check-cross-validation.py`) compares the decoded bytes
//! across every write × read direction pair.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use zarrs::array::codec::api::{
    ArrayBytes, ArrayToArrayCodecTraits, ArrayToBytesCodecTraits, BytesToBytesCodecTraits, Codec,
};
use zarrs::array::{Array, ArrayBuilder, DataType, FillValue, data_type};
use zarrs::filesystem::FilesystemStore;
use zarrs::metadata::v3::MetadataV3;

use ndic_lift::LiftKind;
use ndic_zarr::series::{Axis, Decorrelate, DeltaBackend, Family, SeriesSpec, codec_series};

// Naming the codec types (not just depending on the crate) is what links
// their `inventory` plugin registrations into this binary — the same
// pattern the ndic-zarr test suites use.
#[allow(unused_imports)]
use ndic_zarr::{
    delta_codec::DeltaCodec, htj2k_codec::Htj2kCodec, lift_codec::NdLiftCodec,
    zfp_codec::NdZfpCodec,
};

/// Arguments for `ndic zarr`.
#[derive(clap::Args)]
pub struct ZarrArgs {
    #[command(subcommand)]
    command: ZarrCommand,
}

#[derive(clap::Subcommand)]
enum ZarrCommand {
    /// Create a Zarr v3 array in a filesystem store from a case spec and
    /// raw element bytes, encoding with the registered nd-image-codecs
    /// pipeline.
    Write(WriteArgs),
    /// Decode a Zarr v3 array from a filesystem store back to raw
    /// little-endian element bytes in C order.
    Read(ReadArgs),
}

#[derive(clap::Args)]
struct WriteArgs {
    /// Store directory (created if absent).
    #[arg(long)]
    store: PathBuf,
    /// JSON case spec: `shape`, `axes`, `chunk_shape`, `dtype`, `family`,
    /// and the builder `options` (the fixture-matrix schema).
    #[arg(long)]
    spec: PathBuf,
    /// Raw little-endian C-order element bytes matching `shape` × `dtype`.
    #[arg(long)]
    input: PathBuf,
}

#[derive(clap::Args)]
struct ReadArgs {
    /// Store directory holding a Zarr v3 array at the root.
    #[arg(long)]
    store: PathBuf,
    /// Output file for the decoded raw bytes.
    #[arg(long)]
    output: PathBuf,
}

/// One validation-matrix case: the fixture-matrix schema plus the array
/// `shape` (the matrix only carries chunk shapes).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseSpec {
    /// Case name; informational.
    #[serde(default)]
    #[allow(dead_code)]
    name: Option<String>,
    shape: Vec<u64>,
    axes: Vec<String>,
    chunk_shape: Vec<u64>,
    dtype: String,
    family: String,
    #[serde(default)]
    options: CaseOptions,
}

/// The builder options, named exactly as in `fixtures/codec-series`.
#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CaseOptions {
    #[serde(default)]
    decorrelate: Option<Vec<usize>>,
    #[serde(default)]
    add_decorrelate: Vec<usize>,
    #[serde(default)]
    remove_decorrelate: Vec<usize>,
    #[serde(default)]
    lift: Option<String>,
    #[serde(default)]
    xy_levels: Option<u8>,
    #[serde(default)]
    reversible: Option<bool>,
    #[serde(default)]
    delta_backend: Option<String>,
    #[serde(default)]
    zfp_rate: Option<f64>,
}

impl CaseSpec {
    fn series_spec(&self) -> Result<SeriesSpec> {
        let family = match self.family.as_str() {
            "nd-delta" => Family::NdDelta,
            "nd-lift-ht" => Family::NdLiftHt,
            "nd-zfp" => Family::NdZfp,
            other => bail!("unknown family {other:?}"),
        };
        let axes = self
            .axes
            .iter()
            .enumerate()
            .map(|(i, name)| Axis::new(i, name))
            .collect();
        let mut spec = SeriesSpec::new(axes, self.chunk_shape.clone(), &self.dtype, family);
        let opts = &self.options;
        spec.decorrelate = match &opts.decorrelate {
            Some(exact) => Decorrelate::Exact(exact.clone()),
            None if opts.add_decorrelate.is_empty() && opts.remove_decorrelate.is_empty() => {
                Decorrelate::Defaults
            }
            None => Decorrelate::Adjust {
                add: opts.add_decorrelate.clone(),
                remove: opts.remove_decorrelate.clone(),
            },
        };
        if let Some(lift) = &opts.lift {
            spec.lift = match lift.as_str() {
                "delta" => LiftKind::Delta,
                "haar" => LiftKind::Haar,
                "lift53" => LiftKind::Lift53,
                other => bail!("unknown lift kind {other:?}"),
            };
        }
        if let Some(xy_levels) = opts.xy_levels {
            spec.xy_levels = xy_levels;
        }
        if let Some(reversible) = opts.reversible {
            spec.reversible = reversible;
        }
        if let Some(backend) = &opts.delta_backend {
            spec.delta_backend = match backend.as_str() {
                "zstd" => DeltaBackend::Zstd,
                "lz4" => DeltaBackend::Lz4,
                other => bail!("unknown delta backend {other:?}"),
            };
        }
        spec.zfp_rate = opts.zfp_rate;
        Ok(spec)
    }
}

fn named_data_type(dtype: &str) -> Result<(DataType, usize)> {
    Ok(match dtype {
        "uint8" => (data_type::uint8(), 1),
        "int8" => (data_type::int8(), 1),
        "uint16" => (data_type::uint16(), 2),
        "int16" => (data_type::int16(), 2),
        "uint32" => (data_type::uint32(), 4),
        "int32" => (data_type::int32(), 4),
        "uint64" => (data_type::uint64(), 8),
        "int64" => (data_type::int64(), 8),
        "float32" => (data_type::float32(), 4),
        "float64" => (data_type::float64(), 8),
        other => bail!("unsupported dtype {other:?}"),
    })
}

/// Dispatch for `ndic zarr`.
///
/// # Errors
/// Any spec, I/O, or codec failure, with context.
pub fn run(args: &ZarrArgs) -> Result<()> {
    match &args.command {
        ZarrCommand::Write(write_args) => write(write_args),
        ZarrCommand::Read(read_args) => read(read_args),
    }
}

fn write(args: &WriteArgs) -> Result<()> {
    let spec_text = std::fs::read_to_string(&args.spec)
        .with_context(|| format!("reading spec {}", args.spec.display()))?;
    let case: CaseSpec = serde_json::from_str(&spec_text)
        .with_context(|| format!("parsing spec {}", args.spec.display()))?;
    let (data_type, itemsize) = named_data_type(&case.dtype)?;
    let elements: u64 = case
        .shape
        .iter()
        .try_fold(1u64, |acc, &d| acc.checked_mul(d))
        .with_context(|| format!("shape {:?} overflows u64", case.shape))?;
    let expected_len = usize::try_from(elements)
        .ok()
        .and_then(|n| n.checked_mul(itemsize))
        .with_context(|| format!("shape {:?} x {} overflows usize", case.shape, case.dtype))?;
    let input = std::fs::read(&args.input)
        .with_context(|| format!("reading input {}", args.input.display()))?;
    if input.len() != expected_len {
        bail!(
            "input is {} bytes but shape {:?} × {} needs {expected_len}",
            input.len(),
            case.shape,
            case.dtype
        );
    }

    let codecs = codec_series(&case.series_spec()?).context("building the codec series")?;
    let mut array_to_array: Vec<Arc<dyn ArrayToArrayCodecTraits>> = Vec::new();
    let mut array_to_bytes: Option<Arc<dyn ArrayToBytesCodecTraits>> = None;
    let mut bytes_to_bytes: Vec<Arc<dyn BytesToBytesCodecTraits>> = Vec::new();
    for metadata in codecs {
        let metadata: MetadataV3 = serde_json::from_value(metadata).context("codec metadata")?;
        match Codec::from_metadata(&metadata)
            .with_context(|| format!("constructing codec {}", metadata.name()))?
        {
            Codec::ArrayToArray(codec) => array_to_array.push(codec),
            Codec::ArrayToBytes(codec) => array_to_bytes = Some(codec),
            Codec::BytesToBytes(codec) => bytes_to_bytes.push(codec),
        }
    }
    let array_to_bytes = array_to_bytes.context("the series has no array-to-bytes codec")?;

    let store = Arc::new(
        FilesystemStore::new(&args.store)
            .with_context(|| format!("opening store {}", args.store.display()))?,
    );
    let array = ArrayBuilder::new(
        case.shape.clone(),
        case.chunk_shape.clone(),
        data_type,
        FillValue::new(vec![0u8; itemsize]),
    )
    .array_to_array_codecs(array_to_array)
    .array_to_bytes_codec(array_to_bytes)
    .bytes_to_bytes_codecs(bytes_to_bytes)
    .dimension_names(case.axes.clone().into())
    .build(store, "/")
    .context("building the array")?;
    array.store_metadata().context("writing zarr.json")?;
    array
        .store_array_subset(&array.subset_all(), ArrayBytes::from(input))
        .context("writing chunks")?;
    Ok(())
}

fn read(args: &ReadArgs) -> Result<()> {
    let store = Arc::new(
        FilesystemStore::new(&args.store)
            .with_context(|| format!("opening store {}", args.store.display()))?,
    );
    let array = Array::open(store, "/").context("opening the array")?;
    let bytes: ArrayBytes<'static> = array
        .retrieve_array_subset(&array.subset_all())
        .context("decoding the array")?;
    let fixed = bytes.into_fixed().context("expected fixed-size elements")?;
    std::fs::write(&args.output, fixed)
        .with_context(|| format!("writing output {}", args.output.display()))?;
    Ok(())
}
