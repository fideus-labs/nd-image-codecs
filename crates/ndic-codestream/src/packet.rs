//! Packet header coding (T.800 §B.10 with the T.814 HT length rules),
//! ported from `OpenJPH`'s `ojph_precinct.cpp` for single-layer streams.

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{Error, Result};

use crate::bitio::{HeaderBitReader, HeaderBitWriter};
use crate::tagtree::TagTree;

/// One coded code-block inside a precinct band, encoder side.
#[derive(Debug, Clone, Default)]
pub struct CodedBlock {
    /// Missing MSBs to signal (`K_max - 1` for our encoder).
    pub missing_msbs: u8,
    /// Number of passes (1 for our encoder).
    pub num_passes: u8,
    /// Cleanup-segment length in bytes.
    pub len_cleanup: u32,
    /// Refinement-segment length (0 for our encoder).
    pub len_refinement: u32,
    /// The codeword segment bytes.
    pub data: Vec<u8>,
}

/// The blocks of one band inside a precinct (raster order).
#[derive(Debug, Clone, Default)]
pub struct BandBlocks {
    /// Grid width in blocks.
    pub nx: usize,
    /// Grid height in blocks.
    pub ny: usize,
    /// `nx * ny` entries; `None` = block not included.
    pub blocks: Vec<Option<CodedBlock>>,
}

impl BandBlocks {
    /// True when the band has no code-blocks at all.
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        self.nx == 0 || self.ny == 0
    }
}

/// A parsed block contribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedBlock {
    /// Band slot (index into the precinct's band list).
    pub band: usize,
    /// Block x within the band grid.
    pub bx: usize,
    /// Block y within the band grid.
    pub by: usize,
    /// Signaled missing MSBs (including placeholder-pass adjustment).
    pub missing_msbs: u8,
    /// Effective passes (1..=3).
    pub num_passes: u8,
    /// Cleanup length in bytes.
    pub len_cleanup: u32,
    /// Refinement length in bytes.
    pub len_refinement: u32,
}

/// Writes one packet (header + block bodies) for a precinct with a single
/// quality layer. `bands` lists the precinct's bands in packet order.
#[must_use]
pub fn write_packet(bands: &[BandBlocks]) -> Vec<u8> {
    let any_included = bands.iter().any(|b| b.blocks.iter().any(Option::is_some));
    if !any_included {
        return alloc::vec![0x00]; // empty packet: a single 0 bit
    }

    let mut bb = HeaderBitWriter::new();
    bb.put_bit(1); // non-empty packet

    for band in bands {
        if band.is_degenerate() {
            continue;
        }
        let (nx, ny) = (band.nx, band.ny);
        let num_levels = TagTree::num_levels(nx, ny) as usize;

        // Build the inclusion (1 = absent) and missing-MSB tag trees.
        let mut inc = TagTree::new(nx, ny, 255);
        let mut mmsb = TagTree::new(nx, ny, 255);
        for y in 0..ny {
            for x in 0..nx {
                let b = &band.blocks[y * nx + x];
                inc.set_val(x, y, 0, u8::from(b.is_none()));
                mmsb.set_val(x, y, 0, b.as_ref().map_or(0, |c| c.missing_msbs));
            }
        }
        inc.reduce();
        mmsb.reduce();

        if inc.val(0, 0, num_levels - 1) != 0 {
            // Entirely empty band: a single 0 bit.
            bb.put_bit(0);
            continue;
        }

        for y in 0..ny {
            for x in 0..nx {
                // Inclusion bits down the tag tree.
                for cur_lev in (1..=num_levels).rev() {
                    let levm1 = cur_lev - 1;
                    if inc.flag(x >> levm1, y >> levm1, levm1) == 0 {
                        let skipped = inc.val(x >> levm1, y >> levm1, levm1)
                            - inc.val(x >> cur_lev, y >> cur_lev, cur_lev);
                        bb.put_bits(u32::from(1 - skipped.min(1)), 1);
                        inc.set_flag(x >> levm1, y >> levm1, levm1);
                    }
                    if inc.val(x >> levm1, y >> levm1, levm1) > 0 {
                        break;
                    }
                }

                let Some(cb) = &band.blocks[y * nx + x] else {
                    continue;
                };

                // Missing MSBs down the tag tree.
                for cur_lev in (1..=num_levels).rev() {
                    let levm1 = cur_lev - 1;
                    if mmsb.flag(x >> levm1, y >> levm1, levm1) == 0 {
                        let zeros = mmsb.val(x >> levm1, y >> levm1, levm1)
                            - mmsb.val(x >> cur_lev, y >> cur_lev, cur_lev);
                        bb.put_zeros(u32::from(zeros));
                        bb.put_bit(1);
                        mmsb.set_flag(x >> levm1, y >> levm1, levm1);
                    }
                }

                // Number of passes.
                match cb.num_passes {
                    1 => bb.put_bits(0, 1),
                    2 => bb.put_bits(2, 2),
                    _ => bb.put_bits(12, 4),
                }

                // Lblock and pass lengths (T.814 HT segment rules).
                let bits1 = 32 - cb.len_cleanup.leading_zeros();
                let extra_bit = u32::from(cb.num_passes > 2);
                let bits2 = if cb.num_passes > 1 {
                    32 - cb.len_refinement.leading_zeros()
                } else {
                    0
                };
                let bits = bits1.max(bits2.saturating_sub(extra_bit)).saturating_sub(3);
                bb.put_bits(0xFFFF_FFFE, bits + 1); // `bits` ones then a zero
                bb.put_bits(cb.len_cleanup, bits + 3);
                if cb.num_passes > 1 {
                    bb.put_bits(cb.len_refinement, bits + 3 + extra_bit);
                }
            }
        }
    }

    let mut out = bb.terminate();
    for band in bands {
        for cb in band.blocks.iter().flatten() {
            out.extend_from_slice(&cb.data);
        }
    }
    out
}

/// Band description handed to the parser: grid plus the `K_max` bound.
#[derive(Debug, Clone, Copy)]
pub struct ParseBand {
    /// Grid width in blocks.
    pub nx: usize,
    /// Grid height in blocks.
    pub ny: usize,
    /// Magnitude bound from QCD/QCC; `missing_msbs` must stay below it.
    pub k_max: u8,
}

/// Result of parsing one packet header.
#[derive(Debug, Clone)]
pub struct ParsedPacket {
    /// Included blocks in body order.
    pub blocks: Vec<ParsedBlock>,
    /// Header length in bytes (bodies follow immediately).
    pub header_len: usize,
}

/// Parses a single-layer packet header from `data` (which starts at the
/// packet header and extends at least to the end of the packet).
///
/// `uses_sop`/`uses_eph` mirror the `Scod` flags; an SOP segment before the
/// header is skipped, an EPH after it is consumed.
///
/// # Errors
/// [`Error::Codestream`] on malformed headers.
#[allow(clippy::too_many_lines)]
pub fn parse_packet_header(
    data: &[u8],
    bands: &[ParseBand],
    uses_sop: bool,
    uses_eph: bool,
    offset: usize,
) -> Result<ParsedPacket> {
    let mut start = 0usize;
    if uses_sop && data.len() >= 6 && data[0] == 0xFF && data[1] == 0x91 {
        start = 6; // SOP marker + Lsop(4) + Nsop covered by Lsop = 4
    }
    let mut bb = HeaderBitReader::new(&data[start..]);
    let mut blocks = Vec::new();

    let mut empty_packet = true;
    for (bi, band) in bands.iter().enumerate() {
        if band.nx == 0 || band.ny == 0 {
            continue;
        }
        if empty_packet {
            if bb.read_bit()? == 0 {
                let mut header_len = start + bb.terminate();
                if uses_eph {
                    header_len = consume_eph(data, header_len, offset)?;
                }
                return Ok(ParsedPacket { blocks, header_len });
            }
            empty_packet = false;
        }

        let (nx, ny) = (band.nx, band.ny);
        let num_levels = TagTree::num_levels(nx, ny) as usize;
        let mut inc = TagTree::new(nx, ny, 0);
        let mut mmsb = TagTree::new(nx, ny, 0);

        for y in 0..ny {
            for x in 0..nx {
                // Inclusion.
                let mut empty_cb = false;
                for cl in (1..=num_levels).rev() {
                    let cur_lev = cl - 1;
                    empty_cb = inc.val(x >> cur_lev, y >> cur_lev, cur_lev) == 1;
                    if empty_cb {
                        break;
                    }
                    if inc.flag(x >> cur_lev, y >> cur_lev, cur_lev) == 0 {
                        let bit = bb.read_bit()?;
                        empty_cb = bit == 0;
                        #[allow(clippy::cast_possible_truncation)]
                        inc.set_val(x >> cur_lev, y >> cur_lev, cur_lev, (1 - bit) as u8);
                        inc.set_flag(x >> cur_lev, y >> cur_lev, cur_lev);
                    }
                    if empty_cb {
                        break;
                    }
                }
                if empty_cb {
                    continue;
                }

                // Missing MSBs.
                let mut mmsbs = 0u32;
                for levp1 in (1..=num_levels).rev() {
                    let cur_lev = levp1 - 1;
                    mmsbs = u32::from(mmsb.val(x >> levp1, y >> levp1, levp1));
                    if mmsb.flag(x >> cur_lev, y >> cur_lev, cur_lev) == 0 {
                        loop {
                            let bit = bb.read_bit()?;
                            if bit == 1 {
                                break;
                            }
                            mmsbs += 1;
                        }
                        #[allow(clippy::cast_possible_truncation)]
                        mmsb.set_val(x >> cur_lev, y >> cur_lev, cur_lev, mmsbs.min(255) as u8);
                        mmsb.set_flag(x >> cur_lev, y >> cur_lev, cur_lev);
                    }
                }
                if mmsbs > u32::from(band.k_max) {
                    return Err(Error::Codestream {
                        offset,
                        message: "missing MSBs exceed the QCD K_max bound".into(),
                    });
                }

                // Number of passes.
                let mut num_passes = 1u32;
                if bb.read_bit()? == 1 {
                    num_passes = 2;
                    if bb.read_bit()? == 1 {
                        let v = bb.read_bits(2)?;
                        num_passes = 3 + v;
                        if v == 3 {
                            let v = bb.read_bits(5)?;
                            num_passes = 6 + v;
                            if v == 31 {
                                num_passes = 37 + bb.read_bits(7)?;
                            }
                        }
                    }
                }

                // Placeholder passes shift the cleanup bitplane down.
                let num_phld = (num_passes - 1) / 3;
                let mmsbs = mmsbs + num_phld;
                let num_passes = num_passes - num_phld * 3;

                // Lblock, then the segment lengths.
                let mut lblock = 3u32;
                while bb.read_bit()? == 1 {
                    lblock += 1;
                }
                let bits = lblock + 31 - (num_phld * 3 + 1).leading_zeros();
                let len1 = bb.read_bits(bits)?;
                if !(2..65_535).contains(&len1) {
                    return Err(Error::Codestream {
                        offset,
                        message: "HT cleanup segment length out of range".into(),
                    });
                }
                let mut len2 = 0;
                if num_passes > 1 {
                    let bits = lblock + u32::from(num_passes > 2);
                    len2 = bb.read_bits(bits)?;
                    if len2 >= 2047 {
                        return Err(Error::Codestream {
                            offset,
                            message: "HT refinement segment length out of range".into(),
                        });
                    }
                }

                #[allow(clippy::cast_possible_truncation)]
                blocks.push(ParsedBlock {
                    band: bi,
                    bx: x,
                    by: y,
                    missing_msbs: mmsbs.min(255) as u8,
                    num_passes: num_passes.min(3) as u8,
                    len_cleanup: len1,
                    len_refinement: len2,
                });
            }
        }
    }
    if empty_packet {
        // All bands degenerate: still one bit on the wire.
        let _ = bb.read_bit();
    }

    let mut header_len = start + bb.terminate();
    if uses_eph {
        header_len = consume_eph(data, header_len, offset)?;
    }
    Ok(ParsedPacket { blocks, header_len })
}

fn consume_eph(data: &[u8], pos: usize, offset: usize) -> Result<usize> {
    if data.len() >= pos + 2 && data[pos] == 0xFF && data[pos + 1] == 0x92 {
        Ok(pos + 2)
    } else {
        Err(Error::Codestream {
            offset: offset + pos,
            message: "expected EPH after packet header".into(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::unnecessary_wraps)]
mod tests {
    use super::*;

    fn cb(mmsb: u8, len: u32) -> Option<CodedBlock> {
        Some(CodedBlock {
            missing_msbs: mmsb,
            num_passes: 1,
            len_cleanup: len,
            len_refinement: 0,
            data: alloc::vec![0xAB; len as usize],
        })
    }

    #[test]
    fn empty_packet_is_one_zero_byte() {
        let bands = [BandBlocks {
            nx: 2,
            ny: 2,
            blocks: alloc::vec![None, None, None, None],
        }];
        let bytes = write_packet(&bands);
        assert_eq!(bytes, alloc::vec![0x00]);
        let parsed = parse_packet_header(
            &bytes,
            &[ParseBand {
                nx: 2,
                ny: 2,
                k_max: 10,
            }],
            false,
            false,
            0,
        )
        .unwrap();
        assert!(parsed.blocks.is_empty());
        assert_eq!(parsed.header_len, 1);
    }

    #[test]
    fn roundtrips_various_grids() {
        for (nx, ny) in [(1, 1), (2, 1), (3, 2), (4, 4), (5, 3)] {
            let mut blocks = alloc::vec![None; nx * ny];
            // Include a scattering of blocks with distinct mmsbs/lengths.
            for i in (0..nx * ny).step_by(2) {
                blocks[i] = cb((3 + i % 5) as u8, 2 + i as u32 * 7);
            }
            let bands = [BandBlocks { nx, ny, blocks }];
            let bytes = write_packet(&bands);
            let parsed =
                parse_packet_header(&bytes, &[ParseBand { nx, ny, k_max: 20 }], false, false, 0)
                    .unwrap();
            let included: Vec<_> = (0..nx * ny).filter(|i| i % 2 == 0).collect();
            assert_eq!(parsed.blocks.len(), included.len(), "{nx}x{ny}");
            for (pb, &i) in parsed.blocks.iter().zip(included.iter()) {
                assert_eq!((pb.bx, pb.by), (i % nx, i / nx), "{nx}x{ny}");
                assert_eq!(pb.missing_msbs, (3 + i % 5) as u8);
                assert_eq!(pb.len_cleanup, 2 + i as u32 * 7);
                assert_eq!(pb.num_passes, 1);
            }
            // Body length must match the sum of segment lengths.
            let body: u32 = parsed.blocks.iter().map(|b| b.len_cleanup).sum();
            assert_eq!(bytes.len(), parsed.header_len + body as usize);
        }
    }

    #[test]
    fn three_band_packet_with_empty_band() {
        let bands = [
            BandBlocks {
                nx: 2,
                ny: 1,
                blocks: alloc::vec![cb(2, 9), None],
            },
            BandBlocks {
                nx: 2,
                ny: 1,
                blocks: alloc::vec![None, None],
            },
            BandBlocks {
                nx: 1,
                ny: 1,
                blocks: alloc::vec![cb(0, 300)],
            },
        ];
        let bytes = write_packet(&bands);
        let pb = [
            ParseBand {
                nx: 2,
                ny: 1,
                k_max: 10,
            },
            ParseBand {
                nx: 2,
                ny: 1,
                k_max: 10,
            },
            ParseBand {
                nx: 1,
                ny: 1,
                k_max: 10,
            },
        ];
        let parsed = parse_packet_header(&bytes, &pb, false, false, 0).unwrap();
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].band, 0);
        assert_eq!(parsed.blocks[1].band, 2);
        assert_eq!(parsed.blocks[1].len_cleanup, 300);
    }
}
