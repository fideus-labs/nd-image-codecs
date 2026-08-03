//! The `compress`, `expand` and `inspect` subcommands.

use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::ValueEnum;
use ndic_codestream::{Codestream, jph};
use ndic_core::{CoeffPlane, EncodeParams, ProgressionOrder, SampleType};

use crate::image_io;

/// `--raw-dtype` values.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DtypeArg {
    /// Unsigned 8-bit.
    U8,
    /// Signed 8-bit.
    I8,
    /// Unsigned 16-bit little-endian.
    U16,
    /// Signed 16-bit little-endian.
    I16,
}

impl From<DtypeArg> for SampleType {
    fn from(v: DtypeArg) -> Self {
        match v {
            DtypeArg::U8 => SampleType::U8,
            DtypeArg::I8 => SampleType::I8,
            DtypeArg::U16 => SampleType::U16,
            DtypeArg::I16 => SampleType::I16,
        }
    }
}

/// `--progression` values.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProgArg {
    /// Layer-resolution-component-position.
    Lrcp,
    /// Resolution-layer-component-position.
    Rlcp,
    /// Resolution-position-component-layer (default).
    Rpcl,
    /// Position-component-resolution-layer.
    Pcrl,
    /// Component-position-resolution-layer.
    Cprl,
}

impl From<ProgArg> for ProgressionOrder {
    fn from(v: ProgArg) -> Self {
        match v {
            ProgArg::Lrcp => ProgressionOrder::Lrcp,
            ProgArg::Rlcp => ProgressionOrder::Rlcp,
            ProgArg::Rpcl => ProgressionOrder::Rpcl,
            ProgArg::Pcrl => ProgressionOrder::Pcrl,
            ProgArg::Cprl => ProgressionOrder::Cprl,
        }
    }
}

/// Arguments for `ndic compress`.
#[derive(clap::Args)]
pub struct CompressArgs {
    /// Input image: `.pgm`/`.ppm`, `.png`, or `.raw` (with `--raw-*`).
    #[arg(short, long)]
    pub input: PathBuf,
    /// Output codestream: `.j2c`/`.j2k` (raw) or `.jph` (boxed).
    #[arg(short, long)]
    pub output: PathBuf,
    /// Wavelet decomposition levels.
    #[arg(long, default_value_t = 5)]
    pub levels: u8,
    /// Code-block size as `WxH` (powers of two, area <= 4096).
    #[arg(long, default_value = "64x64")]
    pub block: String,
    /// Progression order.
    #[arg(long, value_enum, default_value = "rpcl")]
    pub progression: ProgArg,
    /// Raw input dimensions as `WxH`.
    #[arg(long)]
    pub raw_size: Option<String>,
    /// Raw input sample type.
    #[arg(long, value_enum)]
    pub raw_dtype: Option<DtypeArg>,
}

/// Arguments for `ndic expand`.
#[derive(clap::Args)]
pub struct ExpandArgs {
    /// Input codestream: `.j2c`/`.j2k`/`.jph`.
    #[arg(short, long)]
    pub input: PathBuf,
    /// Output image: `.pgm`/`.ppm`, `.png`, or `.raw`.
    #[arg(short, long)]
    pub output: PathBuf,
    /// Decode only resolutions `0..=R` (default: all).
    #[arg(long)]
    pub resolution: Option<u8>,
    /// The input is a fetched byte-range prefix (e.g. from `curl -r`):
    /// decode the highest resolution the bytes on hand cover.
    #[arg(long)]
    pub partial: bool,
}

/// Arguments for `ndic inspect`.
#[derive(clap::Args)]
pub struct InspectArgs {
    /// Input codestream: `.j2c`/`.j2k`/`.jph`.
    #[arg(short, long)]
    pub input: PathBuf,
    /// Also list every packet from the TLM/PLT index.
    #[arg(long)]
    pub packets: bool,
}

fn parse_wxh(s: &str) -> anyhow::Result<(usize, usize)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .with_context(|| format!("expected WxH, got {s:?}"))?;
    Ok((w.trim().parse()?, h.trim().parse()?))
}

/// Runs `ndic compress`.
#[allow(clippy::cast_precision_loss)] // ratio display only
pub fn compress(args: &CompressArgs) -> anyhow::Result<()> {
    let raw = match (&args.raw_size, args.raw_dtype) {
        (Some(size), Some(dtype)) => {
            let (w, h) = parse_wxh(size)?;
            Some((w, h, dtype.into()))
        }
        (None, None) => None,
        _ => bail!("--raw-size and --raw-dtype must be given together"),
    };
    let image = image_io::load(&args.input, raw)?;
    let (cbw, cbh) = parse_wxh(&args.block)?;
    let params = EncodeParams {
        xy_levels: args.levels,
        code_block: (
            u16::try_from(cbw).context("block width")?,
            u16::try_from(cbh).context("block height")?,
        ),
        progression: args.progression.into(),
        ..EncodeParams::default()
    };
    let planes: Vec<CoeffPlane> = image
        .comps
        .iter()
        .map(|c| CoeffPlane::new(c, image.width, image.height))
        .collect::<Result<_, _>>()?;
    let stream = ndic_codestream::encode_image(&planes, image.dtype, &params)?;

    let boxed = args
        .output
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jph") || e.eq_ignore_ascii_case("jp2"));
    let bytes = if boxed {
        let comps: Vec<(u8, bool)> = image
            .comps
            .iter()
            .map(|_| (image.dtype.bit_depth(), image.dtype.is_signed()))
            .collect();
        jph::wrap(
            &stream,
            u32::try_from(image.width)?,
            u32::try_from(image.height)?,
            &comps,
        )
    } else {
        stream
    };
    std::fs::write(&args.output, &bytes)
        .with_context(|| format!("writing {}", args.output.display()))?;
    let in_bytes = image.width * image.height * image.comps.len() * image.dtype.size_bytes();
    println!(
        "{} -> {} ({} -> {} bytes, ratio {:.4})",
        args.input.display(),
        args.output.display(),
        in_bytes,
        bytes.len(),
        bytes.len() as f64 / in_bytes as f64,
    );
    Ok(())
}

/// Runs `ndic expand`.
pub fn expand(args: &ExpandArgs) -> anyhow::Result<()> {
    let data =
        std::fs::read(&args.input).with_context(|| format!("reading {}", args.input.display()))?;
    let (cs, dec) = if args.partial {
        // A fetched prefix: headers intact, packet bodies cut short. The
        // PLT index tells which resolutions the bytes on hand cover.
        let cs_bytes = if data.starts_with(&[0xFF, 0x4F]) {
            &data[..]
        } else {
            &data[jph::codestream_offset(&data)?..]
        };
        let cs = Codestream::parse_prefix(cs_bytes)?;
        let entry = ndic_codestream::container::PlaneEntry::from_codestream(&cs, 0)?;
        let covered = entry
            .prefix
            .iter()
            .rposition(|&p| u64::from(p) <= cs_bytes.len() as u64)
            .context("the prefix covers no complete resolution")?;
        let r = args
            .resolution
            .map_or(covered, |r| usize::from(r).min(covered));
        let dec = cs.decode_to_resolution(u8::try_from(r)?)?;
        (cs, dec)
    } else {
        let cs_bytes = jph::unwrap_codestream(&data)?;
        let cs = Codestream::parse(cs_bytes)?;
        let dec = match args.resolution {
            Some(r) => cs.decode_to_resolution(r)?,
            None => cs.decode()?,
        };
        (cs, dec)
    };
    let c0 = cs.siz.comps[0];
    let dtype = match (c0.depth > 8, c0.signed) {
        (false, false) => SampleType::U8,
        (false, true) => SampleType::I8,
        (true, false) => SampleType::U16,
        (true, true) => SampleType::I16,
    };
    let image = image_io::Image {
        comps: dec.comps,
        width: dec.width,
        height: dec.height,
        dtype,
    };
    image_io::save(&args.output, &image)?;
    println!(
        "{} -> {} ({}x{}, {} component(s))",
        args.input.display(),
        args.output.display(),
        image.width,
        image.height,
        image.comps.len(),
    );
    Ok(())
}

/// Runs `ndic inspect`.
pub fn inspect(args: &InspectArgs) -> anyhow::Result<()> {
    let data =
        std::fs::read(&args.input).with_context(|| format!("reading {}", args.input.display()))?;
    if jph::is_box_format(&data) {
        let f = jph::parse(&data)?;
        if let Some(h) = f.header {
            println!(
                "jph box file: ihdr {}x{}, {} component(s), bpc {:#04x}",
                h.width, h.height, h.num_comps, h.bpc
            );
        }
        println!("codestream at bytes {}..{}", f.codestream.0, f.codestream.1);
    }
    let cs_bytes = jph::unwrap_codestream(&data)?;
    let cs = Codestream::parse(cs_bytes)?;

    println!(
        "SIZ: {}x{} Rsiz {:#06x}, tile {}x{}, {} component(s)",
        cs.siz.xsiz,
        cs.siz.ysiz,
        cs.siz.rsiz,
        cs.siz.xtsiz,
        cs.siz.ytsiz,
        cs.siz.comps.len()
    );
    for (i, c) in cs.siz.comps.iter().enumerate() {
        println!(
            "  component {i}: {} bit {}, subsampling {}x{}",
            c.depth,
            if c.signed { "signed" } else { "unsigned" },
            c.xr,
            c.yr
        );
    }
    let prog = ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"]
        .get(usize::from(cs.cod.progression))
        .unwrap_or(&"?");
    println!(
        "COD: {prog}, {} layers, {} levels, block {}x{}, {} wavelet, {} coder{}",
        cs.cod.layers,
        cs.cod.decomps,
        cs.cod.cb_size().0,
        cs.cod.cb_size().1,
        if cs.cod.wavelet == 1 { "5/3" } else { "9/7" },
        if cs.cod.is_ht() { "HT" } else { "EBCOT" },
        if cs.cod.precincts.is_empty() {
            ", maximal precincts".to_owned()
        } else {
            format!(", precincts {:?}", cs.cod.precincts)
        },
    );
    println!(
        "QCD: style {}, {} guard bit(s), {} subband(s)",
        cs.quant.style,
        cs.quant.guard_bits,
        cs.quant.values.len()
    );
    if let Some(cap) = cs.cap {
        println!("CAP: Ccap15 {:#06x} (MAGB bound signalled)", cap.ccap15);
    }
    if let Some(com) = &cs.comment {
        println!("COM: {com}");
    }
    println!("TLM: {:?}; tile-parts: {}", cs.tlm, cs.tile_parts.len());
    for (i, tp) in cs.tile_parts.iter().enumerate() {
        println!(
            "  tile-part {i}: tile {} at byte {}, body {}..{}, {} PLT entr{}",
            tp.sot.isot,
            tp.offset,
            tp.body.start,
            tp.body.end,
            tp.plt.len(),
            if tp.plt.len() == 1 { "y" } else { "ies" }
        );
    }

    match cs.packet_index() {
        Ok(spans) => {
            println!(
                "packet index (from TLM/PLT, no decoding): {} packets",
                spans.len()
            );
            if args.packets {
                for s in &spans {
                    println!(
                        "  r{} c{} p({},{}) @ {:>8} +{}",
                        s.res, s.comp, s.precinct.0, s.precinct.1, s.offset, s.len
                    );
                }
            }
        }
        Err(e) => println!("packet index unavailable: {e}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ndic-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn compress_args(input: &std::path::Path, output: &std::path::Path) -> CompressArgs {
        CompressArgs {
            input: input.into(),
            output: output.into(),
            levels: 3,
            block: "64x64".into(),
            progression: ProgArg::Rpcl,
            raw_size: None,
            raw_dtype: None,
        }
    }

    #[test]
    fn parses_wxh() {
        assert_eq!(parse_wxh("64x64").unwrap(), (64, 64));
        assert_eq!(parse_wxh("1024X4").unwrap(), (1024, 4));
        assert!(parse_wxh("64").is_err());
    }

    #[test]
    fn pgm_jph_roundtrip() {
        let src = tmp("in.pgm");
        let jph = tmp("mid.jph");
        let out = tmp("out.pgm");
        let mut pgm = b"P5\n9 7\n255\n".to_vec();
        pgm.extend((0..63u8).map(|i| i.wrapping_mul(37)));
        std::fs::write(&src, &pgm).unwrap();

        compress(&compress_args(&src, &jph)).unwrap();
        expand(&ExpandArgs {
            input: jph.clone(),
            output: out.clone(),
            resolution: None,
            partial: false,
        })
        .unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&out).unwrap());

        inspect(&InspectArgs {
            input: jph,
            packets: true,
        })
        .unwrap();
    }

    #[test]
    fn raw_roundtrip_and_partial() {
        let src = tmp("in.raw");
        let j2c = tmp("mid.j2c");
        let out = tmp("out.raw");
        let bytes: Vec<u8> = (0..32 * 16 * 2u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &bytes).unwrap();

        let mut args = compress_args(&src, &j2c);
        args.raw_size = Some("32x16".into());
        args.raw_dtype = Some(DtypeArg::U16);
        compress(&args).unwrap();
        expand(&ExpandArgs {
            input: j2c.clone(),
            output: out.clone(),
            resolution: None,
            partial: false,
        })
        .unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&out).unwrap());

        // Partial decode: resolution 2 of 3 levels is the half-size level.
        let half = tmp("half.pgm");
        expand(&ExpandArgs {
            input: j2c,
            output: half.clone(),
            resolution: Some(2),
            partial: false,
        })
        .unwrap();
        let head = std::fs::read(&half).unwrap();
        assert!(head.starts_with(b"P5\n16 8\n"));
    }

    #[test]
    fn png_roundtrip() {
        let src = tmp("in.png");
        let jph = tmp("png.jph");
        let out = tmp("out.png");
        let image = image_io::Image {
            comps: (0..3)
                .map(|c| (0..24 * 10).map(|i| (i * (c + 3)) % 256).collect())
                .collect(),
            width: 24,
            height: 10,
            dtype: ndic_core::SampleType::U8,
        };
        image_io::save(&src, &image).unwrap();
        compress(&compress_args(&src, &jph)).unwrap();
        expand(&ExpandArgs {
            input: jph,
            output: out.clone(),
            resolution: None,
            partial: false,
        })
        .unwrap();
        let back = image_io::load(&out, None).unwrap();
        assert_eq!(back.comps, image.comps);
    }
}
