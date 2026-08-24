//! 2D image file I/O for the `ndic` CLI: PGM/PPM (P5/P6), PNG, and raw
//! little-endian sample dumps.

use std::path::Path;

use anyhow::{Context, bail};
use ndic_core::SampleType;

/// A loaded (or to-be-saved) 2D image: planar components.
pub struct Image {
    /// One plane per component, row-major.
    pub comps: Vec<Vec<i32>>,
    /// Width in samples.
    pub width: usize,
    /// Height in samples.
    pub height: usize,
    /// Sample type of every component.
    pub dtype: SampleType,
}

/// File kind by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Binary PGM (P5) or PPM (P6).
    Pnm,
    /// PNG (via the `png` crate).
    Png,
    /// Headerless little-endian samples.
    Raw,
}

/// Determines a file's format from its extension.
pub fn format_of(path: &Path) -> anyhow::Result<Format> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "pgm" | "ppm" | "pnm" => Ok(Format::Pnm),
        "png" => Ok(Format::Png),
        "raw" | "bin" => Ok(Format::Raw),
        other => bail!("unsupported image extension {other:?} (use pgm/ppm/png/raw)"),
    }
}

/// Loads an image; `raw` supplies `(width, height, dtype)` for raw dumps.
pub fn load(path: &Path, raw: Option<(usize, usize, SampleType)>) -> anyhow::Result<Image> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    match format_of(path)? {
        Format::Pnm => load_pnm(&data),
        Format::Png => load_png(&data),
        Format::Raw => {
            let (width, height, dtype) =
                raw.context("raw input needs --raw-size WxH and --raw-dtype")?;
            load_raw(&data, width, height, dtype)
        }
    }
}

/// Saves an image in the format implied by the extension.
pub fn save(path: &Path, image: &Image) -> anyhow::Result<()> {
    let bytes = match format_of(path)? {
        Format::Pnm => save_pnm(image)?,
        Format::Png => save_png(image)?,
        Format::Raw => save_raw(image)?,
    };
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn load_pnm(data: &[u8]) -> anyhow::Result<Image> {
    let mut fields = Vec::new();
    let mut pos = 0usize;
    while fields.len() < 4 && pos < data.len() {
        while pos < data.len() {
            if data[pos].is_ascii_whitespace() {
                pos += 1;
            } else if data[pos] == b'#' {
                while pos < data.len() && data[pos] != b'\n' {
                    pos += 1;
                }
            } else {
                break;
            }
        }
        let start = pos;
        while pos < data.len() && !data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(std::str::from_utf8(&data[start..pos])?.to_owned());
    }
    if fields.len() < 4 {
        bail!("truncated PNM header (need magic, width, height, maxval)");
    }
    pos += 1; // exactly one whitespace byte after maxval (netpbm raw format)
    let ncomp = match fields.first().map(String::as_str) {
        Some("P5") => 1usize,
        Some("P6") => 3,
        other => bail!("unsupported PNM magic {other:?} (only binary P5/P6)"),
    };
    let width: usize = fields[1].parse()?;
    let height: usize = fields[2].parse()?;
    let maxval: u32 = fields[3].parse()?;
    let dtype = if maxval > 255 {
        SampleType::U16
    } else {
        SampleType::U8
    };
    let n = width * height;
    let body = &data[pos..];
    let mut comps = vec![vec![0i32; n]; ncomp];
    if maxval > 255 {
        if body.len() < 2 * n * ncomp {
            bail!("PNM body truncated");
        }
        for (i, &ch) in body.as_chunks::<2>().0.iter().take(n * ncomp).enumerate() {
            comps[i % ncomp][i / ncomp] = i32::from(u16::from_be_bytes(ch));
        }
    } else {
        if body.len() < n * ncomp {
            bail!("PNM body truncated");
        }
        for (i, &b) in body.iter().take(n * ncomp).enumerate() {
            comps[i % ncomp][i / ncomp] = i32::from(b);
        }
    }
    Ok(Image {
        comps,
        width,
        height,
        dtype,
    })
}

fn save_pnm(image: &Image) -> anyhow::Result<Vec<u8>> {
    let (magic, ncomp) = match image.comps.len() {
        1 => ("P5", 1usize),
        3 => ("P6", 3),
        n => bail!("PNM supports 1 or 3 components, image has {n}"),
    };
    let maxval: u32 = match image.dtype {
        SampleType::U8 => 255,
        SampleType::U16 => 65535,
        other => bail!("PNM cannot store {other:?} samples"),
    };
    let n = image.width * image.height;
    let mut out = format!("{magic}\n{} {}\n{maxval}\n", image.width, image.height).into_bytes();
    for i in 0..n {
        for c in 0..ncomp {
            let v = image.comps[c][i];
            if maxval > 255 {
                out.extend_from_slice(&u16::try_from(v.clamp(0, 65535))?.to_be_bytes());
            } else {
                out.push(u8::try_from(v.clamp(0, 255))?);
            }
        }
    }
    Ok(out)
}

fn load_png(data: &[u8]) -> anyhow::Result<Image> {
    let decoder = png::Decoder::new(data);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let width = info.width as usize;
    let height = info.height as usize;
    let ncomp = match info.color_type {
        png::ColorType::Grayscale => 1usize,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => bail!("indexed PNG is not supported; convert first"),
    };
    let (dtype, bytes_per) = match info.bit_depth {
        png::BitDepth::Eight => (SampleType::U8, 1usize),
        png::BitDepth::Sixteen => (SampleType::U16, 2),
        other => bail!("PNG bit depth {other:?} is not supported"),
    };
    let n = width * height;
    let mut comps = vec![vec![0i32; n]; ncomp];
    for i in 0..n * ncomp {
        let v = if bytes_per == 2 {
            i32::from(u16::from_be_bytes([buf[2 * i], buf[2 * i + 1]]))
        } else {
            i32::from(buf[i])
        };
        comps[i % ncomp][i / ncomp] = v;
    }
    Ok(Image {
        comps,
        width,
        height,
        dtype,
    })
}

fn save_png(image: &Image) -> anyhow::Result<Vec<u8>> {
    let color = match image.comps.len() {
        1 => png::ColorType::Grayscale,
        2 => png::ColorType::GrayscaleAlpha,
        3 => png::ColorType::Rgb,
        4 => png::ColorType::Rgba,
        n => bail!("PNG supports 1..=4 components, image has {n}"),
    };
    let (depth, max) = match image.dtype {
        SampleType::U8 => (png::BitDepth::Eight, 255),
        SampleType::U16 => (png::BitDepth::Sixteen, 65535),
        other => bail!("PNG cannot store {other:?} samples"),
    };
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(
            &mut out,
            u32::try_from(image.width)?,
            u32::try_from(image.height)?,
        );
        enc.set_color(color);
        enc.set_depth(depth);
        let mut writer = enc.write_header()?;
        let n = image.width * image.height;
        let ncomp = image.comps.len();
        let mut bytes = Vec::with_capacity(n * ncomp * 2);
        for i in 0..n {
            for c in 0..ncomp {
                let v = image.comps[c][i].clamp(0, max);
                if max > 255 {
                    bytes.extend_from_slice(&u16::try_from(v)?.to_be_bytes());
                } else {
                    bytes.push(u8::try_from(v)?);
                }
            }
        }
        writer.write_image_data(&bytes)?;
    }
    Ok(out)
}

fn load_raw(data: &[u8], width: usize, height: usize, dtype: SampleType) -> anyhow::Result<Image> {
    let n = width
        .checked_mul(height)
        .context("raw dimensions overflow")?;
    let need = n
        .checked_mul(dtype.size_bytes())
        .context("raw size overflows")?;
    if data.len() < need {
        bail!("raw file holds {} bytes, need {need}", data.len());
    }
    let mut plane = vec![0i32; n];
    for (i, v) in plane.iter_mut().enumerate() {
        *v = match dtype {
            SampleType::U8 => i32::from(data[i]),
            #[allow(clippy::cast_possible_wrap)] // reinterpret the raw byte
            SampleType::I8 => i32::from(data[i] as i8),
            SampleType::U16 => i32::from(u16::from_le_bytes([data[2 * i], data[2 * i + 1]])),
            SampleType::I16 => i32::from(i16::from_le_bytes([data[2 * i], data[2 * i + 1]])),
            SampleType::U32 | SampleType::I32 => i32::from_le_bytes([
                data[4 * i],
                data[4 * i + 1],
                data[4 * i + 2],
                data[4 * i + 3],
            ]),
            _ => bail!("unsupported raw dtype {dtype:?}"),
        };
    }
    Ok(Image {
        comps: vec![plane],
        width,
        height,
        dtype,
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // masked casts
fn save_raw(image: &Image) -> anyhow::Result<Vec<u8>> {
    // Raw output is single-component only: `load_raw` reads exactly one
    // plane, so a multi-component dump would not round-trip. Use PNG or
    // PPM for colour images.
    if image.comps.len() != 1 {
        bail!(
            "raw output supports 1 component, image has {} (use png/ppm)",
            image.comps.len()
        );
    }
    let mut out = Vec::new();
    for &v in &image.comps[0] {
        match image.dtype {
            SampleType::U8 | SampleType::I8 => out.push((v & 0xFF) as u8),
            SampleType::U16 | SampleType::I16 => {
                out.extend_from_slice(&((v & 0xFFFF) as u16).to_le_bytes());
            }
            _ => out.extend_from_slice(&v.to_le_bytes()),
        }
    }
    Ok(out)
}
