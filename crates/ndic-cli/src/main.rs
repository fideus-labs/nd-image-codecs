//! The `ndic` command-line tool.
//!
//! Mirrors `OpenJPH`'s `ojph_compress` / `ojph_expand` split under one binary,
//! plus `series`, `inspect`, `index`, and `thumbnail` subcommands for codec-
//! series generation, codestream introspection, byte-range planning, and
//! thumbnail extraction.

use clap::{Parser, Subcommand, ValueEnum};

mod commands;
mod image_io;
mod plans;
#[cfg(feature = "zarr")]
mod zarr_io;

/// nd-image-codecs command-line interface.
#[derive(Parser)]
#[command(name = "ndic", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compress a 2D PGM/PPM/PNG/raw image to an HTJ2K codestream.
    Compress(commands::CompressArgs),
    /// Expand an HTJ2K codestream back to a 2D PGM/PPM/PNG/raw image.
    Expand(commands::ExpandArgs),
    /// Emit a Zarr v3 codec-series JSON pipeline (nd-delta / nd-lift-ht /
    /// nd-zfp) from axis names, chunk shape, and dtype.
    Series(SeriesArgs),
    /// Print codestream structure (markers, resolutions, layers, tile-parts).
    Inspect(commands::InspectArgs),
    /// Emit a byte-range plan (from TLM/PLT and the coefficient-plane
    /// index) for thumbnail / plane / region fetch, executable by any HTTP
    /// client.
    Index(plans::IndexArgs),
    /// Plan, fetch (local file or HTTP Range), and decode a thumbnail or
    /// low-pass 3D preview in one step.
    Thumbnail(plans::ThumbnailArgs),
    /// Write / read Zarr v3 filesystem stores with the registered
    /// nd-image-codecs pipelines (build with `--features zarr`).
    #[cfg(feature = "zarr")]
    Zarr(zarr_io::ZarrArgs),
}

/// Arguments for `ndic series`.
#[derive(clap::Args)]
struct SeriesArgs {
    /// Parse one Zarr v3 codec object with that codec's own configuration
    /// type and print it back, instead of building a series. Round-tripping
    /// through the codec is how a documented or hand-written configuration
    /// is checked against what the codec actually accepts.
    #[arg(long, conflicts_with = "chunks", value_name = "JSON")]
    validate_codec: Option<String>,
    /// Comma-separated axis names in dimension order, e.g. `t,c,z,y,x`.
    #[arg(long, default_value = "t,c,z,y,x")]
    axes: String,
    /// Comma-separated chunk shape, e.g. `1,1,32,256,256`.
    #[arg(long, required_unless_present = "validate_codec")]
    chunks: Option<String>,
    /// Zarr data type, e.g. `uint16`.
    #[arg(long, default_value = "uint16")]
    dtype: String,
    /// Codec family: `nd-delta`, `nd-lift-ht`, or `nd-zfp`.
    #[arg(long, default_value = "nd-lift-ht")]
    family: String,
    /// Decorrelate exactly these dimension indices (replaces the defaults),
    /// e.g. `--decorrelate 1` for a correlated channel axis.
    #[arg(long, value_delimiter = ',', conflicts_with_all = ["add_decorrelate", "remove_decorrelate"])]
    decorrelate: Option<Vec<usize>>,
    /// Dimension indices to decorrelate in addition to the defaults.
    #[arg(long, value_delimiter = ',')]
    add_decorrelate: Vec<usize>,
    /// Dimension indices to exclude from the default decorrelation set.
    #[arg(long, value_delimiter = ',')]
    remove_decorrelate: Vec<usize>,
    /// Transform kind for decorrelation axes (nd-lift-ht).
    #[arg(long, value_enum, default_value = "lift53")]
    lift: LiftArg,
    /// In-plane resolution levels for the htj2k plane codec.
    #[arg(long, default_value_t = 5)]
    xy_levels: u8,
    /// Lossy coding (default is reversible / lossless).
    #[arg(long)]
    lossy: bool,
    /// Blosc backend for the nd-delta family.
    #[arg(long, value_enum, default_value = "zstd")]
    delta_backend: DeltaBackendArg,
    /// ZFP bits-per-value for nd-zfp (omit for reversible mode).
    #[arg(long)]
    zfp_rate: Option<f64>,
}

/// `--lift` values.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum LiftArg {
    /// First-order differencing.
    Delta,
    /// Reversible Haar lifting.
    Haar,
    /// Reversible 5/3-style lifting.
    Lift53,
}

/// `--delta-backend` values.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum DeltaBackendArg {
    /// Blosc-Zstd (best ratio).
    Zstd,
    /// Blosc-LZ4 (fastest).
    Lz4,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Series(args) => match run_series(&args) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("ndic series: {e}");
                std::process::exit(1);
            }
        },
        Command::Compress(args) => exit_on_error(commands::compress(&args)),
        Command::Expand(args) => exit_on_error(commands::expand(&args)),
        Command::Inspect(args) => exit_on_error(commands::inspect(&args)),
        Command::Index(args) => exit_on_error(plans::index(&args)),
        Command::Thumbnail(args) => exit_on_error(plans::thumbnail(&args)),
        #[cfg(feature = "zarr")]
        Command::Zarr(args) => exit_on_error(zarr_io::run(&args)),
    }
}

fn exit_on_error(result: anyhow::Result<()>) {
    if let Err(e) = result {
        eprintln!("ndic: {e:#}");
        std::process::exit(1);
    }
}

/// Parse one Zarr v3 codec object through the owning codec's configuration
/// type and re-emit it. Each type is `deny_unknown_fields`, so an unknown or
/// misspelled key is an error rather than a silently ignored field.
fn validate_codec(json: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or("a codec object needs a string \"name\"")?;
    let configuration = value
        .get("configuration")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let parsed = match name {
        "nd_lift" => serde_json::to_value(serde_json::from_value::<ndic_lift::NdLiftConfig>(
            configuration,
        )?)?,
        "htj2k" => serde_json::to_value(serde_json::from_value::<ndic_zarr::htj2k::Htj2kConfig>(
            configuration,
        )?)?,
        // `zfp` is the registered name; `nd_zfp` predates the adoption and
        // stays readable (its configurations may carry the legacy `dims`).
        "zfp" | "nd_zfp" => serde_json::to_value(serde_json::from_value::<ndic_zfp::NdZfpConfig>(
            configuration,
        )?)?,
        other => {
            return Err(
                format!("{other:?} is not an nd-image-codecs codec (nd_lift, htj2k, zfp)").into(),
            );
        }
    };
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "name": name,
        "configuration": parsed,
    }))?)
}

fn run_series(args: &SeriesArgs) -> Result<String, Box<dyn std::error::Error>> {
    use ndic_lift::LiftKind;
    use ndic_zarr::series::{Axis, Decorrelate, DeltaBackend, Family, SeriesSpec, codec_series};
    if let Some(json) = &args.validate_codec {
        return validate_codec(json);
    }
    let axes: Vec<Axis> = args
        .axes
        .split(',')
        .enumerate()
        .map(|(i, n)| Axis::new(i, n.trim()))
        .collect();
    let chunks: Vec<u64> = args
        .chunks
        .as_deref()
        .ok_or("--chunks is required")?
        .split(',')
        .map(|c| c.trim().parse())
        .collect::<Result<_, _>>()?;
    let family = match args.family.as_str() {
        "nd-delta" => Family::NdDelta,
        "nd-lift-ht" => Family::NdLiftHt,
        "nd-zfp" => Family::NdZfp,
        other => return Err(format!("unknown family {other:?}").into()),
    };
    let mut spec = SeriesSpec::new(axes, chunks, &args.dtype, family);
    spec.decorrelate = match &args.decorrelate {
        Some(exact) => Decorrelate::Exact(exact.clone()),
        None if !args.add_decorrelate.is_empty() || !args.remove_decorrelate.is_empty() => {
            Decorrelate::Adjust {
                add: args.add_decorrelate.clone(),
                remove: args.remove_decorrelate.clone(),
            }
        }
        None => Decorrelate::Defaults,
    };
    spec.lift = match args.lift {
        LiftArg::Delta => LiftKind::Delta,
        LiftArg::Haar => LiftKind::Haar,
        LiftArg::Lift53 => LiftKind::Lift53,
    };
    spec.xy_levels = args.xy_levels;
    spec.reversible = !args.lossy;
    spec.delta_backend = match args.delta_backend {
        DeltaBackendArg::Zstd => DeltaBackend::Zstd,
        DeltaBackendArg::Lz4 => DeltaBackend::Lz4,
    };
    spec.zfp_rate = args.zfp_rate;
    let codecs = codec_series(&spec)?;
    Ok(serde_json::to_string_pretty(&codecs)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(argv: &[&str]) -> Result<serde_json::Value, String> {
        let cli =
            Cli::try_parse_from([&["ndic", "series"], argv].concat()).map_err(|e| e.to_string())?;
        let Command::Series(parsed) = cli.command else {
            unreachable!("series subcommand");
        };
        let json = run_series(&parsed).map_err(|e| e.to_string())?;
        Ok(serde_json::from_str(&json).expect("valid JSON output"))
    }

    #[test]
    fn emits_all_three_families() {
        for (family, tail) in [
            ("nd-delta", "blosc"),
            ("nd-lift-ht", "htj2k"),
            ("nd-zfp", "zfp"),
        ] {
            let codecs = series(&["--chunks", "1,1,32,256,256", "--family", family]).unwrap();
            let names: Vec<_> = codecs
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["name"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(
                names.last().map(String::as_str),
                Some(tail),
                "{family}: {names:?}"
            );
        }
    }

    #[test]
    fn decorrelate_override_and_lift_kind() {
        let codecs = series(&[
            "--chunks",
            "1,4,32,256,256",
            "--decorrelate",
            "1",
            "--lift",
            "haar",
        ])
        .unwrap();
        let tf = &codecs[0]["configuration"]["transforms"][0];
        assert_eq!(tf["axis"], "c");
        assert_eq!(tf["kind"], "haar");
    }

    #[test]
    fn adjust_flags_and_lossy_float() {
        let codecs = series(&[
            "--axes",
            "z,y,x",
            "--chunks",
            "32,256,256",
            "--dtype",
            "float32",
            "--lossy",
            "--xy-levels",
            "3",
            "--remove-decorrelate",
            "0",
        ])
        .unwrap();
        assert_eq!(codecs[0]["name"], "htj2k");
        assert_eq!(codecs[0]["configuration"]["xy_levels"], 3);
        assert_eq!(codecs[0]["configuration"]["reversible"], false);
    }

    #[test]
    fn delta_backend_and_zfp_rate() {
        let codecs = series(&[
            "--chunks",
            "1,1,32,256,256",
            "--family",
            "nd-delta",
            "--delta-backend",
            "lz4",
        ])
        .unwrap();
        let blosc = codecs.as_array().unwrap().last().unwrap().clone();
        assert_eq!(blosc["configuration"]["cname"], "lz4");

        let codecs = series(&[
            "--axes",
            "z,y,x",
            "--chunks",
            "32,256,256",
            "--dtype",
            "float32",
            "--family",
            "nd-zfp",
            "--zfp-rate",
            "8",
        ])
        .unwrap();
        assert_eq!(codecs[0]["configuration"]["mode"], "fixed_rate");
        assert_eq!(codecs[0]["configuration"]["rate"], 8.0);
    }

    #[test]
    fn invalid_input_is_an_error() {
        assert!(series(&["--chunks", "32,256,256", "--axes", "z,y,w"]).is_err());
        assert!(series(&["--chunks", "1,1", "--family", "no-such-family"]).is_err());
    }
}
