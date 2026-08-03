//! The `htj2k` chunk container: `[header | coefficient-plane index |
//! codestreams…]`.
//!
//! A Zarr chunk compressed by the `htj2k` array-to-bytes codec holds one
//! independent Part 1/15 codestream per trailing 2D plane. This module owns
//! the byte layout that binds them together: a small fixed header describing
//! the chunk shape, then — when the codec's `index` option is on — the
//! **coefficient-plane index**: per plane, its byte range within the chunk
//! and the prefix length that covers each resolution (computed from the
//! codestream's `TLM`/`PLT`). [`crate::range::RangeIndex`] turns this into
//! byte-range plans; the header parses from a chunk *prefix* so a remote
//! reader bootstraps from one small ranged fetch.
//!
//! All integers little-endian. Layout (version 1):
//!
//! ```text
//! 0        4   magic  "ndht"
//! 4        1   version = 1
//! 5        1   flags   (bit 0: coefficient-plane index present)
//! 6        1   xy_levels
//! 7        1   ndim    (2..=32)
//! 8    4*ndim  dims, u32 each — transformed order, trailing two = (y, x)
//! ...          index: num_planes entries, each
//!                u64 offset | u32 len | (xy_levels + 1) * u32 prefix
//! ...          codestreams, in plane order
//! ```
//!
//! `prefix[r]` is the byte count from the plane's first byte that suffices
//! to decode resolutions `0..=r` (main header + tile-part header + the
//! leading packets) via [`crate::reader::Codestream::parse_prefix`].

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{Error, Result};

/// Magic bytes opening every `htj2k` chunk.
pub const MAGIC: [u8; 4] = *b"ndht";
/// The container version this build writes and reads.
pub const VERSION: u8 = 1;
/// Flag bit 0: the coefficient-plane index follows the fixed header.
pub const FLAG_INDEX: u8 = 0x01;

/// Most dimensions a chunk may declare (Zarr arrays in the wild are ≤ 5-6D;
/// this bounds hostile headers).
pub const MAX_NDIM: usize = 32;
/// Most planes a chunk may declare — bounds hostile headers before the
/// entry table is sized (2^24 planes of even 1 KiB would be a 16 GiB chunk).
pub const MAX_PLANES: u64 = 1 << 24;

fn err(offset: usize, message: &str) -> Error {
    Error::Codestream {
        offset,
        message: message.into(),
    }
}

/// One plane's slot in the coefficient-plane index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneEntry {
    /// Byte offset of the plane's codestream from the chunk's first byte.
    pub offset: u64,
    /// Byte length of the plane's codestream.
    pub len: u32,
    /// `prefix[r]`: bytes from the plane's first byte that decode
    /// resolutions `0..=r`. Length `xy_levels + 1`, non-decreasing,
    /// `prefix[xy_levels] <= len`.
    pub prefix: Vec<u32>,
}

impl PlaneEntry {
    /// Builds the entry for a parsed codestream placed at `offset` within
    /// the chunk: length from the declared stream extent, per-resolution
    /// prefix lengths from the `TLM`/`PLT` packet index.
    ///
    /// # Errors
    /// See [`Codestream::packet_index`](crate::reader::Codestream::packet_index);
    /// [`Error::Unsupported`] when the stream exceeds the 4 GiB plane bound.
    pub fn from_codestream(cs: &crate::reader::Codestream<'_>, offset: u64) -> Result<Self> {
        let spans = cs.packet_index()?;
        let too_big = || Error::Unsupported {
            message: "codestream exceeds the 4 GiB plane index bound".into(),
        };
        let len = u32::try_from(cs.total_len()).map_err(|_| too_big())?;
        let prefix = (0..=cs.cod.decomps)
            .map(|r| {
                spans
                    .iter()
                    .filter(|s| s.res <= r)
                    .map(|s| u32::try_from(s.offset + s.len).map_err(|_| too_big()))
                    .try_fold(0u32, |acc, end| end.map(|e| acc.max(e)))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            offset,
            len,
            prefix,
        })
    }
}

/// The parsed (or to-be-written) chunk header and coefficient-plane index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkHeader {
    /// Chunk shape in transformed order; `dims[ndim-2]` is the plane height
    /// (y) and `dims[ndim-1]` the plane width (x).
    pub dims: Vec<u32>,
    /// In-plane decomposition levels every plane was coded with.
    pub xy_levels: u8,
    /// The coefficient-plane index, when written (`index: true`).
    pub planes: Option<Vec<PlaneEntry>>,
}

impl ChunkHeader {
    /// Number of trailing 2D planes: the product of the leading dims.
    #[must_use]
    pub fn num_planes(&self) -> usize {
        self.dims[..self.dims.len() - 2]
            .iter()
            .map(|&d| d as usize)
            .product()
    }

    /// Plane height (the `y` extent).
    #[must_use]
    pub fn plane_height(&self) -> u32 {
        self.dims[self.dims.len() - 2]
    }

    /// Plane width (the `x` extent).
    #[must_use]
    pub fn plane_width(&self) -> u32 {
        self.dims[self.dims.len() - 1]
    }

    /// Bytes of the fixed header for `ndim` dimensions.
    #[must_use]
    pub fn fixed_len(ndim: usize) -> usize {
        8 + 4 * ndim
    }

    /// Bytes of one index entry at `xy_levels`.
    #[must_use]
    pub fn entry_len(xy_levels: u8) -> usize {
        8 + 4 + 4 * (usize::from(xy_levels) + 1)
    }

    /// Total bytes of `[header | index]` — the offset of the first
    /// codestream when the index is present.
    #[must_use]
    pub fn header_len(&self) -> usize {
        Self::fixed_len(self.dims.len())
            + self
                .planes
                .as_ref()
                .map_or(0, |p| p.len() * Self::entry_len(self.xy_levels))
    }

    /// Whether `bytes` opens with the container magic.
    #[must_use]
    pub fn is_container(bytes: &[u8]) -> bool {
        bytes.len() >= 4 && bytes[..4] == MAGIC
    }

    /// Total `[header | index]` length declared by a chunk prefix of at
    /// least [`ChunkHeader::fixed_len`]`(ndim)` bytes — how much a remote
    /// reader must fetch before [`ChunkHeader::parse`] succeeds.
    ///
    /// # Errors
    /// [`Error::Codestream`] when the prefix is shorter than the fixed
    /// header or the header is malformed.
    pub fn required_len(bytes: &[u8]) -> Result<usize> {
        let (ndim, xy_levels, flags) = Self::parse_fixed(bytes)?;
        if bytes.len() < Self::fixed_len(ndim) {
            return Err(err(bytes.len(), "chunk header prefix too short"));
        }
        let dims = Self::parse_dims(bytes, ndim)?;
        let num_planes = Self::checked_num_planes(&dims)?;
        let index_len = if flags & FLAG_INDEX == 0 {
            0
        } else {
            num_planes * Self::entry_len(xy_levels)
        };
        Ok(Self::fixed_len(ndim) + index_len)
    }

    fn parse_fixed(bytes: &[u8]) -> Result<(usize, u8, u8)> {
        if bytes.len() < 8 {
            return Err(err(0, "chunk header prefix too short"));
        }
        if bytes[..4] != MAGIC {
            return Err(err(0, "not an htj2k chunk (bad magic)"));
        }
        if bytes[4] != VERSION {
            return Err(err(4, "unsupported htj2k chunk container version"));
        }
        let flags = bytes[5];
        if flags & !FLAG_INDEX != 0 {
            return Err(err(5, "unknown htj2k chunk container flags"));
        }
        let xy_levels = bytes[6];
        if xy_levels > 33 {
            return Err(err(6, "xy_levels exceeds the 33-level J2K bound"));
        }
        let ndim = usize::from(bytes[7]);
        if !(2..=MAX_NDIM).contains(&ndim) {
            return Err(err(7, "chunk ndim outside 2..=32"));
        }
        Ok((ndim, xy_levels, flags))
    }

    fn parse_dims(bytes: &[u8], ndim: usize) -> Result<Vec<u32>> {
        let mut dims = Vec::with_capacity(ndim);
        for i in 0..ndim {
            let at = 8 + 4 * i;
            let d = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
            if d == 0 {
                return Err(err(at, "zero chunk dimension"));
            }
            dims.push(d);
        }
        Ok(dims)
    }

    fn checked_num_planes(dims: &[u32]) -> Result<usize> {
        let mut n: u64 = 1;
        for &d in &dims[..dims.len() - 2] {
            n = n
                .checked_mul(u64::from(d))
                .filter(|&n| n <= MAX_PLANES)
                .ok_or_else(|| err(8, "chunk declares more planes than supported"))?;
        }
        usize::try_from(n).map_err(|_| err(8, "chunk declares more planes than supported"))
    }

    /// Serializes `[header | index]`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.header_len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(if self.planes.is_some() { FLAG_INDEX } else { 0 });
        out.push(self.xy_levels);
        #[allow(clippy::cast_possible_truncation)] // ndim <= MAX_NDIM
        out.push(self.dims.len() as u8);
        for &d in &self.dims {
            out.extend_from_slice(&d.to_le_bytes());
        }
        if let Some(planes) = &self.planes {
            for p in planes {
                out.extend_from_slice(&p.offset.to_le_bytes());
                out.extend_from_slice(&p.len.to_le_bytes());
                for &l in &p.prefix {
                    out.extend_from_slice(&l.to_le_bytes());
                }
            }
        }
        out
    }

    /// Parses `[header | index]` from a chunk (or a chunk prefix of at
    /// least [`ChunkHeader::required_len`] bytes).
    ///
    /// # Errors
    /// [`Error::Codestream`] on truncated or malformed headers.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (ndim, xy_levels, flags) = Self::parse_fixed(bytes)?;
        let fixed = Self::fixed_len(ndim);
        if bytes.len() < fixed {
            return Err(err(bytes.len(), "truncated chunk header"));
        }
        let dims = Self::parse_dims(bytes, ndim)?;
        let num_planes = Self::checked_num_planes(&dims)?;

        let planes = if flags & FLAG_INDEX == 0 {
            None
        } else {
            let entry = Self::entry_len(xy_levels);
            let need = fixed + num_planes * entry;
            if bytes.len() < need {
                return Err(err(bytes.len(), "truncated coefficient-plane index"));
            }
            let header_end = need as u64;
            let mut planes = Vec::with_capacity(num_planes);
            let mut min_offset = header_end;
            for i in 0..num_planes {
                let at = fixed + i * entry;
                let e = &bytes[at..at + entry];
                let offset = u64::from_le_bytes(e[..8].try_into().expect("8 bytes"));
                let len = u32::from_le_bytes(e[8..12].try_into().expect("4 bytes"));
                let prefix: Vec<u32> = e[12..]
                    .chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().expect("4 bytes")))
                    .collect();
                if offset < min_offset {
                    return Err(err(at, "plane offsets overlap or precede the index"));
                }
                if prefix.windows(2).any(|w| w[0] > w[1]) || prefix.last().is_some_and(|&l| l > len)
                {
                    return Err(err(at, "plane prefix lengths are inconsistent"));
                }
                min_offset = offset
                    .checked_add(u64::from(len))
                    .ok_or_else(|| err(at, "plane byte range overflows"))?;
                planes.push(PlaneEntry {
                    offset,
                    len,
                    prefix,
                });
            }
            Some(planes)
        };

        Ok(Self {
            dims,
            xy_levels,
            planes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChunkHeader {
        ChunkHeader {
            dims: alloc::vec![4, 16, 32],
            xy_levels: 2,
            planes: Some(
                (0u64..4)
                    .map(|i| PlaneEntry {
                        // header_len = 8 + 4*3 fixed + 4 entries of 24 = 116.
                        offset: 116 + 100 * i,
                        len: 100,
                        prefix: alloc::vec![40, 70, 100],
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn round_trips() {
        let h = sample();
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), h.header_len());
        // A fixed-header prefix is enough to learn the full index length.
        let fixed = ChunkHeader::fixed_len(3);
        assert_eq!(
            ChunkHeader::required_len(&bytes[..fixed]).unwrap(),
            bytes.len()
        );
        assert_eq!(ChunkHeader::parse(&bytes).unwrap(), h);
    }

    #[test]
    fn round_trips_without_index() {
        let h = ChunkHeader {
            planes: None,
            ..sample()
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), ChunkHeader::fixed_len(3));
        assert_eq!(ChunkHeader::parse(&bytes).unwrap(), h);
    }

    #[test]
    fn rejects_hostile_headers() {
        let good = sample().to_bytes();
        // Bad magic.
        let mut b = good.clone();
        b[0] = b'x';
        assert!(ChunkHeader::parse(&b).is_err());
        // Future version.
        let mut b = good.clone();
        b[4] = 2;
        assert!(ChunkHeader::parse(&b).is_err());
        // Unknown flags.
        let mut b = good.clone();
        b[5] = 0x80;
        assert!(ChunkHeader::parse(&b).is_err());
        // ndim outside bounds.
        let mut b = good.clone();
        b[7] = 1;
        assert!(ChunkHeader::parse(&b).is_err());
        // Zero dimension.
        let mut b = good.clone();
        b[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert!(ChunkHeader::parse(&b).is_err());
        // Overlapping plane ranges.
        let mut h = sample();
        h.planes.as_mut().unwrap()[1].offset = 150;
        assert!(ChunkHeader::parse(&h.to_bytes()).is_err());
        // Decreasing prefix lengths.
        let mut h = sample();
        h.planes.as_mut().unwrap()[0].prefix = alloc::vec![70, 40, 100];
        assert!(ChunkHeader::parse(&h.to_bytes()).is_err());
        // Prefix beyond the plane length.
        let mut h = sample();
        h.planes.as_mut().unwrap()[0].prefix = alloc::vec![40, 70, 101];
        assert!(ChunkHeader::parse(&h.to_bytes()).is_err());
        // Truncated index.
        assert!(ChunkHeader::parse(&good[..good.len() - 1]).is_err());
        // Every 1-byte truncation of the fixed header errs, never panics.
        for cut in 0..ChunkHeader::fixed_len(3) {
            assert!(ChunkHeader::parse(&good[..cut]).is_err());
        }
    }

    #[test]
    fn plane_count_overflow_is_caught() {
        let h = ChunkHeader {
            dims: alloc::vec![u32::MAX, u32::MAX, 16, 32],
            xy_levels: 2,
            planes: None,
        };
        let mut bytes = h.to_bytes();
        bytes[5] = FLAG_INDEX; // pretend an index follows
        assert!(ChunkHeader::parse(&bytes).is_err());
        assert!(ChunkHeader::required_len(&bytes).is_err());
    }
}
