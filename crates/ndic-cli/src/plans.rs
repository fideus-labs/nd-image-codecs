//! The `index` and `thumbnail` subcommands: byte-range planning and
//! execution over local files or HTTP (plain `Range:` requests — any
//! static file server or object store).
//!
//! `index` prints a plan any HTTP client can execute (`--format curl`
//! emits the `Range:` header value verbatim); `thumbnail` plans, fetches,
//! and decodes in one step. Both bootstrap from a small prefix fetch:
//! the `htj2k` chunk header + coefficient-plane index, or a bare
//! codestream's main + tile-part headers (`TLM`/`PLT`).

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::ValueEnum;
use ndic_codestream::container::ChunkHeader;
use ndic_codestream::range::{Plan, RangeIndex};
use ndic_codestream::{Codestream, jph};
use ndic_core::SampleType;
use ndic_lift::AxisTransform;

use crate::image_io;

/// `--target` values for `ndic index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TargetArg {
    /// Low-resolution prefix of the first (low-pass-most) plane.
    Thumbnail,
    /// Low-resolution prefixes of each group's low-pass planes.
    #[value(name = "thumbnail-3d")]
    Thumbnail3d,
    /// One full plane by index (`--z`).
    Plane,
    /// Precinct-aligned packets covering `--rect` at `--level`.
    Region,
}

/// `--format` values for `ndic index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    /// The full plan as JSON.
    Json,
    /// The `Range:` header value (`start-end,start-end`) for `curl -H`.
    Curl,
}

/// Arguments for `ndic index`.
#[derive(clap::Args)]
pub struct IndexArgs {
    /// Input `htj2k` chunk / `.j2c` / `.jph`: a local path or http(s) URL.
    pub input: String,
    /// What the plan fetches.
    #[arg(long, value_enum, default_value = "thumbnail")]
    pub target: TargetArg,
    /// Pixel budget for the decoded long side (thumbnail targets).
    #[arg(long, default_value_t = 256)]
    pub max: usize,
    /// Plane index for `--target plane`.
    #[arg(long)]
    pub z: Option<usize>,
    /// Full-resolution `x,y,w,h` rectangle for `--target region`.
    #[arg(long)]
    pub rect: Option<String>,
    /// Resolution ceiling for `--target region`.
    #[arg(long, default_value_t = 0)]
    pub level: u8,
    /// Codec-series JSON (inline or `@file`), as `ndic series` emits it;
    /// its `nd_lift` transforms select the low-pass planes for
    /// `--target thumbnail-3d`.
    #[arg(long)]
    pub series: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value = "json")]
    pub format: FormatArg,
}

/// Arguments for `ndic thumbnail`.
#[derive(clap::Args)]
pub struct ThumbnailArgs {
    /// Input `htj2k` chunk / `.j2c` / `.jph`: a local path or http(s) URL.
    pub input: String,
    /// Pixel budget for the decoded long side.
    #[arg(long, default_value_t = 256)]
    pub max: usize,
    /// A 3D preview from the low-pass planes (raw output only).
    #[arg(long)]
    pub three_d: bool,
    /// Codec-series JSON (inline or `@file`) selecting the low-pass planes
    /// for `--three-d`; omit when the chunk was not decorrelated.
    #[arg(long)]
    pub series: Option<String>,
    /// Output image (`.pgm`/`.png`/`.raw`; `--three-d` needs `.raw`).
    #[arg(short, long)]
    pub output: PathBuf,
}

/// How much to fetch before the first parse attempt, and the growth cap.
const PROBE_LEN: u64 = 64 * 1024;
const PROBE_CAP: u64 = 64 * 1024 * 1024;

/// A byte source answering exact-range reads: a seekable file or an HTTP
/// endpoint via `Range:` requests (one request per planned range).
enum Source {
    File(std::fs::File),
    Http {
        agent: ureq::Agent,
        url: String,
        /// The whole body, cached when the server ignores `Range:`.
        body: Option<Vec<u8>>,
    },
}

impl Source {
    fn open(input: &str) -> anyhow::Result<Self> {
        if input.starts_with("http://") || input.starts_with("https://") {
            Ok(Self::Http {
                agent: ureq::Agent::new_with_defaults(),
                url: input.to_owned(),
                body: None,
            })
        } else {
            Ok(Self::File(
                std::fs::File::open(input).with_context(|| format!("opening {input}"))?,
            ))
        }
    }

    /// Fetches `[start, end]` (inclusive), clamped at EOF.
    fn fetch(&mut self, start: u64, end: u64) -> anyhow::Result<Vec<u8>> {
        let len = end - start + 1;
        match self {
            Self::File(file) => {
                let total = file.metadata()?.len();
                if start >= total {
                    return Ok(Vec::new());
                }
                let len = len.min(total - start);
                file.seek(SeekFrom::Start(start))?;
                let mut buf = vec![0u8; usize::try_from(len)?];
                file.read_exact(&mut buf)?;
                Ok(buf)
            }
            Self::Http { agent, url, body } => {
                if let Some(body) = body {
                    let s = usize::try_from(start.min(body.len() as u64))?;
                    let e = usize::try_from(end.saturating_add(1).min(body.len() as u64))?;
                    return Ok(body[s..e].to_vec());
                }
                let mut response = agent
                    .get(url.as_str())
                    .header("Range", &format!("bytes={start}-{end}"))
                    .call()
                    .with_context(|| format!("GET {url} (bytes={start}-{end})"))?;
                let bytes = response
                    .body_mut()
                    .with_config()
                    // 200 responses carry the whole file; cap the fallback.
                    .limit(1 << 31)
                    .read_to_vec()
                    .with_context(|| format!("reading {url}"))?;
                if response.status() == 206 {
                    Ok(bytes)
                } else {
                    // No range support: keep the body and slice locally.
                    *body = Some(bytes);
                    self.fetch(start, end)
                }
            }
        }
    }

    /// Fetches the file prefix `[0, want)`, clamped at EOF.
    fn fetch_prefix(&mut self, want: u64) -> anyhow::Result<Vec<u8>> {
        self.fetch(0, want - 1)
    }
}

/// What probing classified the input as.
enum Kind {
    /// An `htj2k` chunk container.
    Chunk,
    /// A bare or boxed codestream; ranges shift by the box offset.
    Codestream { box_offset: u64 },
}

/// A probed input: enough prefix bytes to plan against.
struct Probed {
    kind: Kind,
    index: RangeIndex,
    /// The prefix fetched so far (from byte 0).
    prefix: Vec<u8>,
}

fn probe(source: &mut Source) -> anyhow::Result<Probed> {
    let mut prefix = source.fetch_prefix(PROBE_LEN)?;
    if prefix.is_empty() {
        bail!("input is empty");
    }

    if ChunkHeader::is_container(&prefix) {
        let need = u64::try_from(ChunkHeader::required_len(&prefix)?)?;
        if need > prefix.len() as u64 {
            prefix = source.fetch_prefix(need)?;
        }
        let index = RangeIndex::from_chunk_prefix(&prefix).context(
            "chunk has no coefficient-plane index (index: false); \
             fetch it whole and decode with the zarr codec instead",
        )?;
        return Ok(Probed {
            kind: Kind::Chunk,
            index,
            prefix,
        });
    }

    // A codestream, bare or boxed: grow the prefix until the main and
    // tile-part headers (with PLT) parse.
    loop {
        let box_offset = if jph::is_box_format(&prefix) {
            jph::codestream_offset(&prefix).map(u64::try_from)??
        } else if prefix.starts_with(&[0xFF, 0x4F]) {
            0
        } else {
            bail!("input is neither an htj2k chunk nor a JPEG 2000 codestream");
        };
        let headers = &prefix[usize::try_from(box_offset)?..];
        if let Ok(cs) = Codestream::parse_prefix(headers)
            && !cs.tile_parts.is_empty()
            && let Ok(index) = RangeIndex::from_codestream(&cs)
        {
            return Ok(Probed {
                kind: Kind::Codestream { box_offset },
                index,
                prefix,
            });
        }
        let grown = (prefix.len() as u64) * 4;
        if grown > PROBE_CAP || prefix.len() as u64 >= PROBE_CAP {
            bail!("could not locate complete codestream headers in the first {PROBE_CAP} bytes");
        }
        let next = source.fetch_prefix(grown)?;
        if next.len() == prefix.len() {
            // EOF: the whole input is on hand; parse errors are terminal.
            let cs = Codestream::parse_prefix(&next[usize::try_from(box_offset)?..])?;
            let index = RangeIndex::from_codestream(&cs)?;
            return Ok(Probed {
                kind: Kind::Codestream { box_offset },
                index,
                prefix: next,
            });
        }
        prefix = next;
    }
}

/// `nd_lift` transforms from `--series` (inline JSON or `@file`).
fn lift_steps(series: Option<&str>) -> anyhow::Result<Vec<AxisTransform>> {
    let Some(series) = series else {
        return Ok(Vec::new());
    };
    let text = if let Some(path) = series.strip_prefix('@') {
        std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?
    } else {
        series.to_owned()
    };
    let codecs: serde_json::Value = serde_json::from_str(&text).context("parsing --series")?;
    let Some(codecs) = codecs.as_array() else {
        bail!("--series must be the codec JSON array `ndic series` emits");
    };
    for codec in codecs {
        if codec["name"] == "nd_lift" {
            let config: ndic_lift::NdLiftConfig =
                serde_json::from_value(codec["configuration"].clone())
                    .context("parsing nd_lift configuration in --series")?;
            config.validate_semantics()?;
            return Ok(config.transforms);
        }
    }
    Ok(Vec::new())
}

fn parse_rect(rect: &str) -> anyhow::Result<(u32, u32, u32, u32)> {
    let parts: Vec<u32> = rect
        .split(',')
        .map(|p| p.trim().parse::<u32>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("expected --rect x,y,w,h, got {rect:?}"))?;
    let [x, y, w, h] = parts[..] else {
        bail!("expected --rect x,y,w,h, got {rect:?}");
    };
    Ok((x, y, w, h))
}

fn build_plan(probed: &Probed, args: &IndexArgs) -> anyhow::Result<Plan> {
    let plan = match args.target {
        TargetArg::Thumbnail => probed.index.thumbnail(args.max)?,
        TargetArg::Thumbnail3d => probed
            .index
            .thumbnail_3d(args.max, &lift_steps(args.series.as_deref())?)?,
        TargetArg::Plane => {
            let z = args.z.context("--target plane needs --z")?;
            probed.index.plane(z)?
        }
        TargetArg::Region => {
            let Kind::Codestream { box_offset } = probed.kind else {
                bail!(
                    "region plans address one codestream; on a chunk, plan \
                     `--target plane --z K` first and region-plan the fetched plane"
                );
            };
            let rect = parse_rect(
                args.rect
                    .as_deref()
                    .context("--target region needs --rect")?,
            )?;
            let cs = Codestream::parse_prefix(&probed.prefix[usize::try_from(box_offset)?..])?;
            RangeIndex::region(&cs, rect, args.level)?
        }
    };
    let shift = match probed.kind {
        Kind::Chunk => 0,
        Kind::Codestream { box_offset } => box_offset,
    };
    Ok(plan.shifted(shift))
}

fn plan_json(plan: &Plan) -> serde_json::Value {
    serde_json::json!({
        "target": plan.target,
        "decoded_size": plan.decoded_size,
        "max_res": plan.max_res,
        "planes": plan.planes,
        "ranges": plan.ranges.iter()
            .map(|r| serde_json::json!({ "start": r.start, "end": r.end }))
            .collect::<Vec<_>>(),
        "total_bytes": plan.total_bytes,
    })
}

fn curl_ranges(plan: &Plan) -> String {
    plan.ranges
        .iter()
        .map(|r| format!("{}-{}", r.start, r.end))
        .collect::<Vec<_>>()
        .join(",")
}

/// Runs `ndic index`.
pub fn index(args: &IndexArgs) -> anyhow::Result<()> {
    let mut source = Source::open(&args.input)?;
    let probed = probe(&mut source)?;
    let plan = build_plan(&probed, args)?;
    match args.format {
        FormatArg::Json => println!("{}", serde_json::to_string_pretty(&plan_json(&plan))?),
        FormatArg::Curl => println!("{}", curl_ranges(&plan)),
    }
    Ok(())
}

/// Fetched plan ranges, answering "give me the bytes at [start, start+len)".
struct Fetched {
    blocks: Vec<(u64, Vec<u8>)>,
}

impl Fetched {
    fn fetch(source: &mut Source, plan: &Plan) -> anyhow::Result<Self> {
        let blocks = plan
            .ranges
            .iter()
            .map(|r| Ok((r.start, source.fetch(r.start, r.end)?)))
            .collect::<anyhow::Result<_>>()?;
        Ok(Self { blocks })
    }

    fn slice(&self, start: u64, len: u64) -> anyhow::Result<&[u8]> {
        for (base, bytes) in &self.blocks {
            if start >= *base && start + len <= base + bytes.len() as u64 {
                let at = usize::try_from(start - base)?;
                return Ok(&bytes[at..at + usize::try_from(len)?]);
            }
        }
        bail!("plan ranges do not cover bytes {start}..{}", start + len)
    }
}

/// The narrowest CLI dtype covering a decoded component's declaration.
fn dtype_of(depth: u8, signed: bool) -> SampleType {
    match (depth, signed) {
        (0..=8, false) => SampleType::U8,
        (0..=8, true) => SampleType::I8,
        (9..=16, false) => SampleType::U16,
        (9..=16, true) => SampleType::I16,
        (_, false) => SampleType::U32,
        (_, true) => SampleType::I32,
    }
}

/// Runs `ndic thumbnail`.
pub fn thumbnail(args: &ThumbnailArgs) -> anyhow::Result<()> {
    let mut source = Source::open(&args.input)?;
    let probed = probe(&mut source)?;
    let index_args = IndexArgs {
        input: args.input.clone(),
        target: if args.three_d {
            TargetArg::Thumbnail3d
        } else {
            TargetArg::Thumbnail
        },
        max: args.max,
        z: None,
        rect: None,
        level: 0,
        series: args.series.clone(),
        format: FormatArg::Json,
    };
    let plan = build_plan(&probed, &index_args)?;
    let fetched = Fetched::fetch(&mut source, &plan)?;
    let shift = match probed.kind {
        Kind::Chunk => 0,
        Kind::Codestream { box_offset } => box_offset,
    };

    // Decode each planned plane's prefix.
    let mut planes = Vec::with_capacity(plan.planes.len());
    let mut siz = None;
    for &p in &plan.planes {
        let entry = probed.index.plane_entry(p).context("plane in plan")?;
        let prefix_len =
            u64::from(entry.prefix[usize::from(plan.max_res.min(probed.index.xy_levels()))]);
        let bytes = fetched.slice(shift + entry.offset, prefix_len)?;
        let cs = Codestream::parse_prefix(bytes)?;
        let decoded = cs.decode_to_resolution(plan.max_res)?;
        siz.get_or_insert((cs.siz.comps[0].depth, cs.siz.comps[0].signed));
        planes.push(decoded);
    }
    let (depth, signed) = siz.context("plan selected no planes")?;
    let dtype = dtype_of(depth, signed);
    let (height, width) = (planes[0].height, planes[0].width);

    if args.three_d {
        if image_io::format_of(&args.output)? != image_io::Format::Raw {
            bail!("--three-d output is a volume; use a .raw output path");
        }
        let mut out = Vec::new();
        for decoded in &planes {
            for &v in &decoded.comps[0] {
                // Truncating casts are exact: the decoder bounds samples to
                // the declared depth `dtype_of` sized the element from.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                match dtype.size_bytes() {
                    1 => out.push(v as u8),
                    2 => out.extend_from_slice(&(v as u16).to_le_bytes()),
                    _ => out.extend_from_slice(&v.to_le_bytes()),
                }
            }
        }
        std::fs::write(&args.output, &out)
            .with_context(|| format!("writing {}", args.output.display()))?;
        println!(
            "{} -> {} ({} plane(s) of {width}x{height}, {dtype:?} LE, {} bytes fetched)",
            args.input,
            args.output.display(),
            planes.len(),
            plan.total_bytes,
        );
    } else {
        let decoded = planes.into_iter().next().context("plan selected a plane")?;
        let image = image_io::Image {
            comps: decoded.comps,
            width,
            height,
            dtype,
        };
        image_io::save(&args.output, &image)?;
        println!(
            "{} -> {} ({width}x{height}, {} bytes fetched of the full stream)",
            args.input,
            args.output.display(),
            plan.total_bytes,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_parses() {
        assert_eq!(parse_rect("1,2,3,4").unwrap(), (1, 2, 3, 4));
        assert!(parse_rect("1,2,3").is_err());
        assert!(parse_rect("a,b,c,d").is_err());
    }

    #[test]
    fn lift_steps_come_from_series_json() {
        let series = r#"[
            { "name": "transpose", "configuration": { "order": [1, 0, 2, 3] } },
            { "name": "nd_lift", "configuration": {
                "version": "0.1",
                "transforms": [
                    { "axis": "z", "dimension": 1, "kind": "lift53", "levels": 2, "group": 0 }
                ] } },
            { "name": "htj2k", "configuration": {} }
        ]"#;
        let steps = lift_steps(Some(series)).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].dimension, 1);
        assert!(lift_steps(None).unwrap().is_empty());
        assert!(lift_steps(Some("[]")).unwrap().is_empty());
        assert!(lift_steps(Some("not json")).is_err());
    }
}
