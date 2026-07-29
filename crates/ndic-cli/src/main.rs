//! The `ndic` command-line tool.
//!
//! Mirrors `OpenJPH`'s `ojph_compress` / `ojph_expand` split under one binary,
//! plus `series`, `inspect`, `index`, and `thumbnail` subcommands for codec-
//! series generation, codestream introspection, byte-range planning, and
//! thumbnail extraction.

use clap::{Parser, Subcommand};

/// nd-image-codecs command-line interface.
#[derive(Parser)]
#[command(name = "ndic", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compress a raw / `NIfTI` / OME-Zarr input to HTJ2K coefficient planes.
    Compress,
    /// Expand HTJ2K coefficient planes back to a raw volume or slice.
    Expand,
    /// Emit a Zarr v3 codec-series JSON pipeline (nd-delta / nd-lift-ht /
    /// nd-zfp) from axis names, chunk shape, and dtype.
    Series(SeriesArgs),
    /// Print codestream structure (markers, resolutions, layers, tile-parts).
    Inspect,
    /// Emit the byte-range index (from TLM/PLT and the coefficient-plane
    /// index) for thumbnail / region fetch planning.
    Index,
    /// Extract an XY / XYZ / XYT / XYZT thumbnail from stored low-pass bands.
    Thumbnail,
}

/// Arguments for `ndic series`.
#[derive(clap::Args)]
struct SeriesArgs {
    /// Comma-separated axis names in dimension order, e.g. `t,c,z,y,x`.
    #[arg(long, default_value = "t,c,z,y,x")]
    axes: String,
    /// Comma-separated chunk shape, e.g. `1,1,32,256,256`.
    #[arg(long)]
    chunks: String,
    /// Zarr data type, e.g. `uint16`.
    #[arg(long, default_value = "uint16")]
    dtype: String,
    /// Codec family: `nd-delta`, `nd-lift-ht`, or `nd-zfp`.
    #[arg(long, default_value = "nd-lift-ht")]
    family: String,
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
        Command::Compress
        | Command::Expand
        | Command::Inspect
        | Command::Index
        | Command::Thumbnail => {
            eprintln!("ndic: subcommands are scaffolded; see docs/development/roadmap/");
        }
    }
}

fn run_series(args: &SeriesArgs) -> Result<String, Box<dyn std::error::Error>> {
    use ndic_zarr::series::{Axis, Family, SeriesSpec, codec_series};
    let axes: Vec<Axis> = args
        .axes
        .split(',')
        .enumerate()
        .map(|(i, n)| Axis::new(i, n.trim()))
        .collect();
    let chunks: Vec<u64> = args
        .chunks
        .split(',')
        .map(|c| c.trim().parse())
        .collect::<Result<_, _>>()?;
    let family = match args.family.as_str() {
        "nd-delta" => Family::NdDelta,
        "nd-lift-ht" => Family::NdLiftHt,
        "nd-zfp" => Family::NdZfp,
        other => return Err(format!("unknown family {other:?}").into()),
    };
    let spec = SeriesSpec::new(axes, chunks, &args.dtype, family);
    let codecs = codec_series(&spec)?;
    Ok(serde_json::to_string_pretty(&codecs)?)
}
