//! The `htj2k` chunk codec core: chunk bytes ⇄ `[header | coefficient-plane
//! index | codestreams…]`.
//!
//! Feature-free on purpose: the `zarrs` codec (`htj2k_codec`), the Python
//! (pyo3) binding, and the WASM (TypeScript) binding all call these
//! functions, so every ecosystem produces byte-identical chunks.
//!
//! A chunk's trailing two dimensions are the 2D plane `(y, x)`; each plane
//! becomes an independent single-component RPCL Part 1/15 codestream via
//! [`ndic_codestream::writer::encode_image_with_depth`]. Every plane
//! declares its **actual** dynamic range in `Ssiz` (not the storage dtype's
//! nominal width), which is what lets the `int32` coefficient planes
//! `nd_lift` hands down fit the 32-bit HT datapath. The chunk layout and
//! index live in [`ndic_codestream::container`].

use ndic_core::{CoeffPlane, EncodeParams, Error, ProgressionOrder, Result, SampleType};

use ndic_codestream::container::{ChunkHeader, PlaneEntry};
use ndic_codestream::reader::Codestream;
use ndic_codestream::writer::encode_image_with_depth;

/// The `htj2k` codec `configuration` object (Zarr v3), shared verbatim by
/// the Rust, Python, and TypeScript builders.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Htj2kConfig {
    /// In-plane (2D) decomposition levels; resolutions = `xy_levels + 1`.
    #[serde(default = "default_xy_levels")]
    pub xy_levels: u8,
    /// Lossless 5/3 coding. `false` (lossy 9/7) is not yet implemented.
    #[serde(default = "default_true")]
    pub reversible: bool,
    /// Progression order; `RPCL` is what makes byte-range prefixes
    /// meaningful, and the default.
    #[serde(default = "default_progression")]
    pub progression: String,
    /// Write the coefficient-plane index (and per-plane `TLM`/`PLT`).
    #[serde(default = "default_true")]
    pub index: bool,
}

fn default_xy_levels() -> u8 {
    5
}
fn default_true() -> bool {
    true
}
fn default_progression() -> String {
    "RPCL".into()
}

impl Default for Htj2kConfig {
    fn default() -> Self {
        Self {
            xy_levels: default_xy_levels(),
            reversible: true,
            progression: default_progression(),
            index: true,
        }
    }
}

impl Htj2kConfig {
    /// Validates the configuration's structure.
    ///
    /// `reversible: false` (lossy 9/7) passes here — the config is
    /// well-formed and arrays carrying it must stay openable — and is
    /// refused by [`encode_chunk`] until the lossy path lands.
    ///
    /// # Errors
    /// [`Error::Unsupported`] for an unknown progression;
    /// [`Error::InvalidArgument`] for out-of-range levels.
    pub fn validate(&self) -> Result<()> {
        if self.xy_levels > 32 {
            return Err(Error::InvalidArgument {
                message: "htj2k: xy_levels exceeds the 32-level J2K bound (T.800 SPcod)".into(),
            });
        }
        self.progression_order().map(|_| ())
    }

    /// The parsed progression order.
    ///
    /// # Errors
    /// [`Error::Unsupported`] for an unknown progression name.
    pub fn progression_order(&self) -> Result<ProgressionOrder> {
        match self.progression.as_str() {
            "LRCP" => Ok(ProgressionOrder::Lrcp),
            "RLCP" => Ok(ProgressionOrder::Rlcp),
            "RPCL" => Ok(ProgressionOrder::Rpcl),
            "PCRL" => Ok(ProgressionOrder::Pcrl),
            "CPRL" => Ok(ProgressionOrder::Cprl),
            other => Err(Error::Unsupported {
                message: format!("htj2k: unknown progression order {other:?}"),
            }),
        }
    }

    fn encode_params(&self) -> Result<EncodeParams> {
        Ok(EncodeParams {
            xy_levels: self.xy_levels,
            progression: self.progression_order()?,
            emit_tlm_plt: self.index,
            ..EncodeParams::default()
        })
    }
}

fn invalid(message: String) -> Error {
    Error::InvalidArgument { message }
}

/// Chunk geometry checks shared by encode and decode: 2..=32 dimensions,
/// all extents non-zero, element count fits the byte buffer.
///
/// The bindings (Python, WASM) pass shapes straight from the caller, so
/// nothing upstream guarantees these — a zero extent would make the
/// byte-length check vacuous while `num_planes` stays an unchecked product,
/// and an over-dimensioned shape could not serialize a parseable header.
fn plane_geometry(
    shape: &[usize],
    dtype: SampleType,
    byte_len: usize,
) -> Result<(usize, usize, usize)> {
    if shape.len() < 2 {
        return Err(invalid(format!(
            "htj2k needs a chunk with trailing 2D (y, x) planes; got {} dimension(s)",
            shape.len()
        )));
    }
    if shape.len() > ndic_codestream::container::MAX_NDIM {
        return Err(invalid(format!(
            "htj2k chunks carry at most {} dimensions; got {}",
            ndic_codestream::container::MAX_NDIM,
            shape.len()
        )));
    }
    if let Some(pos) = shape.iter().position(|&d| d == 0) {
        return Err(invalid(format!(
            "htj2k needs non-zero chunk extents; dimension {pos} of {shape:?} is 0"
        )));
    }
    let height = shape[shape.len() - 2];
    let width = shape[shape.len() - 1];
    let num_planes = shape[..shape.len() - 2].iter().product::<usize>();
    let elements = shape
        .iter()
        .try_fold(1usize, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| invalid("chunk element count overflows".into()))?;
    let expect = elements
        .checked_mul(dtype.size_bytes())
        .ok_or_else(|| invalid("chunk byte length overflows".into()))?;
    if byte_len != expect {
        return Err(invalid(format!(
            "chunk of shape {shape:?} and dtype {dtype:?} needs {expect} bytes, got {byte_len}"
        )));
    }
    Ok((num_planes, height, width))
}

/// Reads one plane's native-endian elements as `i32` samples.
fn widen_plane(bytes: &[u8], dtype: SampleType) -> Result<Vec<i32>> {
    let size = dtype.size_bytes();
    debug_assert_eq!(bytes.len() % size, 0);
    let out: Result<Vec<i32>> = match dtype {
        SampleType::U8 => Ok(bytes.iter().map(|&b| i32::from(b)).collect()),
        #[allow(clippy::cast_possible_wrap)]
        SampleType::I8 => Ok(bytes.iter().map(|&b| i32::from(b as i8)).collect()),
        SampleType::U16 => Ok(bytes
            .chunks_exact(2)
            .map(|c| i32::from(u16::from_ne_bytes([c[0], c[1]])))
            .collect()),
        SampleType::I16 => Ok(bytes
            .chunks_exact(2)
            .map(|c| i32::from(i16::from_ne_bytes([c[0], c[1]])))
            .collect()),
        SampleType::U32 => bytes
            .chunks_exact(4)
            .map(|c| {
                let v = u32::from_ne_bytes([c[0], c[1], c[2], c[3]]);
                i32::try_from(v).map_err(|_| {
                    invalid(format!(
                        "uint32 sample {v} exceeds the 32-bit HT datapath; \
                         decorrelate or reduce the dynamic range"
                    ))
                })
            })
            .collect(),
        SampleType::I32 => Ok(bytes
            .chunks_exact(4)
            .map(|c| i32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect()),
        _ => Err(Error::Unsupported {
            message: format!("htj2k does not support dtype {dtype:?}"),
        }),
    };
    out
}

/// Writes decoded `i32` samples back as native-endian `dtype` elements,
/// range-checking (a corrupt chunk must error, not wrap).
pub fn narrow_samples(samples: &[i32], dtype: SampleType, out: &mut Vec<u8>) -> Result<()> {
    fn check(lo: i64, hi: i64, samples: &[i32], dtype: SampleType) -> Result<()> {
        let (min, max) = samples.iter().fold((i64::MAX, i64::MIN), |(lo, hi), &v| {
            (lo.min(i64::from(v)), hi.max(i64::from(v)))
        });
        if !samples.is_empty() && (min < lo || max > hi) {
            return Err(Error::Codestream {
                offset: 0,
                message: format!(
                    "decoded sample range [{min}, {max}] does not fit dtype {dtype:?} \
                     (corrupt or mismatched chunk)"
                ),
            });
        }
        Ok(())
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    match dtype {
        SampleType::U8 => {
            check(0, 0xFF, samples, dtype)?;
            out.extend(samples.iter().map(|&v| v as u8));
        }
        SampleType::I8 => {
            check(i64::from(i8::MIN), i64::from(i8::MAX), samples, dtype)?;
            out.extend(samples.iter().map(|&v| (v as i8) as u8));
        }
        SampleType::U16 => {
            check(0, 0xFFFF, samples, dtype)?;
            for &v in samples {
                out.extend_from_slice(&(v as u16).to_ne_bytes());
            }
        }
        SampleType::I16 => {
            check(i64::from(i16::MIN), i64::from(i16::MAX), samples, dtype)?;
            for &v in samples {
                out.extend_from_slice(&(v as i16).to_ne_bytes());
            }
        }
        SampleType::U32 => {
            check(0, i64::from(u32::MAX), samples, dtype)?;
            for &v in samples {
                out.extend_from_slice(&(v as u32).to_ne_bytes());
            }
        }
        SampleType::I32 => {
            for &v in samples {
                out.extend_from_slice(&v.to_ne_bytes());
            }
        }
        _ => {
            return Err(Error::Unsupported {
                message: format!("htj2k does not support dtype {dtype:?}"),
            });
        }
    }
    Ok(())
}

/// The narrowest `Ssiz` declaration covering the samples' actual range.
fn depth_for(samples: &[i32], signed: bool) -> u8 {
    let (lo, hi) = samples.iter().fold((0i64, 0i64), |(lo, hi), &v| {
        (lo.min(i64::from(v)), hi.max(i64::from(v)))
    });
    for b in 1..=32u8 {
        let fits = if signed {
            lo >= -(1i64 << (b - 1)) && hi < (1i64 << (b - 1))
        } else {
            hi < (1i64 << b)
        };
        if fits {
            return b;
        }
    }
    32
}

/// Encodes a chunk (native-endian `dtype` elements, C order, trailing dims
/// `(y, x)`) into the `htj2k` container.
///
/// # Errors
/// [`Error::InvalidArgument`] on geometry/byte-length mismatches,
/// [`Error::Unsupported`] when a plane's dynamic range exceeds the HT
/// datapath, the dtype has no integer path, or the configuration requests
/// unimplemented coding.
pub fn encode_chunk(
    bytes: &[u8],
    shape: &[usize],
    dtype: SampleType,
    config: &Htj2kConfig,
) -> Result<Vec<u8>> {
    config.validate()?;
    if !config.reversible {
        return Err(Error::Unsupported {
            message: "htj2k: lossy (reversible: false) coding lands with a later phase".into(),
        });
    }
    let params = config.encode_params()?;
    let (num_planes, height, width) = plane_geometry(shape, dtype, bytes.len())?;
    let plane_bytes = height * width * dtype.size_bytes();
    let signed = dtype.is_signed();

    let mut codestreams = Vec::with_capacity(num_planes);
    for p in 0..num_planes {
        let samples = widen_plane(&bytes[p * plane_bytes..(p + 1) * plane_bytes], dtype)?;
        let depth = depth_for(&samples, signed);
        let plane = CoeffPlane::new(&samples, width, height)?;
        let stream =
            encode_image_with_depth(&[plane], depth, signed, &params).map_err(|e| match e {
                Error::Unsupported { message } => Error::Unsupported {
                    message: format!("htj2k plane {p}: {message}"),
                },
                other => other,
            })?;
        codestreams.push(stream);
    }

    let dims = shape
        .iter()
        .map(|&d| u32::try_from(d).map_err(|_| invalid(format!("chunk extent {d} exceeds u32"))))
        .collect::<Result<Vec<u32>>>()?;
    let mut header = ChunkHeader {
        dims,
        xy_levels: config.xy_levels,
        planes: None,
    };
    if config.index {
        // Offsets need the index size, which needs the plane count only.
        let mut offset = ChunkHeader::fixed_len(shape.len()) as u64
            + (num_planes * ChunkHeader::entry_len(config.xy_levels)) as u64;
        let mut entries = Vec::with_capacity(num_planes);
        for stream in &codestreams {
            let cs = Codestream::parse(stream)?;
            let entry = PlaneEntry::from_codestream(&cs, offset)?;
            offset += u64::from(entry.len);
            entries.push(entry);
        }
        header.planes = Some(entries);
    }

    let mut out = header.to_bytes();
    for stream in &codestreams {
        out.extend_from_slice(stream);
    }
    Ok(out)
}

/// Parses a chunk's header and returns each plane's `(offset, len)` within
/// `chunk` — from the index when present, else by walking the concatenated
/// codestreams.
///
/// # Errors
/// [`Error::Codestream`] on malformed containers.
pub fn plane_ranges(chunk: &[u8]) -> Result<(ChunkHeader, Vec<(usize, usize)>)> {
    let header = ChunkHeader::parse(chunk)?;
    let num_planes = header.num_planes();
    let mut ranges = Vec::with_capacity(num_planes);
    if let Some(planes) = &header.planes {
        for p in planes {
            let start = usize::try_from(p.offset)
                .map_err(|_| invalid("plane offset exceeds usize".into()))?;
            let end = start
                .checked_add(p.len as usize)
                .filter(|&e| e <= chunk.len())
                .ok_or_else(|| Error::Codestream {
                    offset: start,
                    message: "plane byte range exceeds the chunk".into(),
                })?;
            ranges.push((start, end - start));
        }
    } else {
        let mut offset = header.header_len();
        for _ in 0..num_planes {
            let rest = chunk.get(offset..).ok_or_else(|| Error::Codestream {
                offset,
                message: "chunk ends before all planes".into(),
            })?;
            let len = Codestream::parse(rest)?.total_len();
            ranges.push((offset, len));
            offset += len;
        }
    }
    Ok((header, ranges))
}

/// Decodes one plane's codestream to `i32` samples, checking geometry
/// against the chunk header.
///
/// # Errors
/// [`Error::Codestream`] on malformed planes or geometry drift.
pub fn decode_plane(header: &ChunkHeader, plane_bytes: &[u8]) -> Result<Vec<i32>> {
    let decoded = Codestream::parse(plane_bytes)?.decode()?;
    if decoded.width != header.plane_width() as usize
        || decoded.height != header.plane_height() as usize
        || decoded.comps.len() != 1
    {
        return Err(Error::Codestream {
            offset: 0,
            message: format!(
                "plane decodes to {}x{}x{}, chunk header declares {}x{}x1",
                decoded.comps.len(),
                decoded.height,
                decoded.width,
                header.plane_height(),
                header.plane_width()
            ),
        });
    }
    Ok(decoded.comps.into_iter().next().expect("one component"))
}

/// Decodes an `htj2k` chunk back to native-endian `dtype` elements in C
/// order.
///
/// # Errors
/// [`Error::Codestream`] on malformed chunks or a shape/dtype mismatch.
pub fn decode_chunk(chunk: &[u8], shape: &[usize], dtype: SampleType) -> Result<Vec<u8>> {
    let (header, ranges) = plane_ranges(chunk)?;
    if header.dims.len() != shape.len()
        || header
            .dims
            .iter()
            .zip(shape)
            .any(|(&d, &s)| d as usize != s)
    {
        return Err(Error::Codestream {
            offset: 0,
            message: format!(
                "chunk header declares shape {:?}, decode expects {shape:?}",
                header.dims
            ),
        });
    }
    let elements = shape.iter().product::<usize>();
    let mut out = Vec::with_capacity(elements * dtype.size_bytes());
    for &(offset, len) in &ranges {
        let samples = decode_plane(&header, &chunk[offset..offset + len])?;
        narrow_samples(&samples, dtype, &mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp_u16(shape: &[usize]) -> Vec<u8> {
        let n: usize = shape.iter().product();
        (0..n)
            .flat_map(|i| u16::try_from((i * 7) % 4096).expect("< 4096").to_ne_bytes())
            .collect()
    }

    #[test]
    fn chunk_round_trips_u16() {
        let shape = [4, 33, 61];
        let bytes = ramp_u16(&shape);
        let config = Htj2kConfig {
            xy_levels: 3,
            ..Htj2kConfig::default()
        };
        let chunk = encode_chunk(&bytes, &shape, SampleType::U16, &config).unwrap();
        assert!(ChunkHeader::is_container(&chunk));
        let back = decode_chunk(&chunk, &shape, SampleType::U16).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn chunk_round_trips_without_index() {
        let shape = [3, 16, 16];
        let bytes = ramp_u16(&shape);
        let config = Htj2kConfig {
            index: false,
            xy_levels: 2,
            ..Htj2kConfig::default()
        };
        let chunk = encode_chunk(&bytes, &shape, SampleType::U16, &config).unwrap();
        let (header, ranges) = plane_ranges(&chunk).unwrap();
        assert!(header.planes.is_none());
        assert_eq!(ranges.len(), 3);
        let back = decode_chunk(&chunk, &shape, SampleType::U16).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn int32_planes_with_narrow_range_encode() {
        // The post-nd_lift shape: int32 storage, small actual values.
        let shape = [2, 16, 16];
        let values: Vec<i32> = (0..512).map(|i| (i % 61) - 30).collect();
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
        let chunk = encode_chunk(&bytes, &shape, SampleType::I32, &Htj2kConfig::default()).unwrap();
        let back = decode_chunk(&chunk, &shape, SampleType::I32).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn full_range_int32_is_refused_with_context() {
        let shape = [1, 4, 4];
        let values: Vec<i32> = (0..16).map(|i| i32::MAX - i).collect();
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
        let err =
            encode_chunk(&bytes, &shape, SampleType::I32, &Htj2kConfig::default()).unwrap_err();
        assert!(err.to_string().contains("htj2k plane 0"), "{err}");
    }

    #[test]
    fn config_defaults_match_the_series_builder() {
        let config: Htj2kConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, Htj2kConfig::default());
        assert_eq!(config.xy_levels, 5);
        assert!(config.reversible && config.index);
        assert_eq!(config.progression, "RPCL");
        // Unknown fields refuse to parse.
        assert!(serde_json::from_str::<Htj2kConfig>(r#"{"tiles": 2}"#).is_err());
        // Lossy configs are well-formed (arrays carrying them stay
        // openable) but encode is honestly refused, not silently lossless.
        let lossy = Htj2kConfig {
            reversible: false,
            ..Htj2kConfig::default()
        };
        assert!(lossy.validate().is_ok());
        let err = encode_chunk(&[0u8; 16], &[4, 4], SampleType::U8, &lossy).unwrap_err();
        assert!(err.to_string().contains("lossy"), "{err}");
    }
}
