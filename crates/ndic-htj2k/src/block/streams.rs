//! Sub-bitstream readers and writers for the HT cleanup / refinement passes.
//!
//! Three stream disciplines exist inside an HT codeword segment
//! (T.814 §7.3.2):
//!
//! - **forward, `0xFF`-stuffed** — `MagSgn` (fed `0xFF` at exhaustion) and
//!   `SigProp` (fed zeros): [`FwdReader`], [`MagSgnEncoder`];
//! - **backward, `>0x8F`-stuffed** — VLC and `MagRef`: [`RevReader`],
//!   [`VlcEncoder`];
//! - the MEL stream (see [`super::mel`]).
//!
//! Ported from `OpenJPH` `ojph_block_decoder32.cpp` / `ojph_block_encoder.cpp`
//! (BSD-2-Clause), with bounds-checked reads instead of the original's
//! deliberate out-of-buffer reads.

extern crate alloc;

use alloc::vec::Vec;

/// Reader for a backward-growing, backward-read bitstream (VLC, `MagRef`).
///
/// Bytes are consumed from the end of the segment toward its start;
/// bits accumulate LSB-first. Unstuffing: when the previously read byte
/// (higher address) is `> 0x8F` and the current byte's low 7 bits are
/// `0x7F`, the current byte contributes only 7 bits.
#[derive(Debug)]
pub struct RevReader<'a> {
    data: &'a [u8],
    /// Next read position (may run past the segment start; reads then fill 0).
    pos: isize,
    /// Bytes remaining.
    size: i32,
    /// Bit accumulator; the next bits are at the bottom.
    tmp: u64,
    /// Number of valid bits in [`Self::tmp`].
    bits: u32,
    /// True if the last consumed byte was `> 0x8F`.
    unstuff: bool,
}

impl<'a> RevReader<'a> {
    /// Creates the VLC reader of a cleanup segment.
    ///
    /// The VLC stream ends at `lcup - 2`; the high nibble of that byte is the
    /// first VLC data, the low nibble and the following byte carry `Scup`.
    pub fn new_vlc(coded: &'a [u8], lcup: usize, scup: usize) -> Self {
        debug_assert!(scup >= 2 && scup <= lcup && lcup <= coded.len());
        let d = coded[lcup - 2];
        let tmp = u64::from(d >> 4);
        let mut r = Self {
            data: coded,
            #[allow(clippy::cast_possible_wrap)]
            pos: (lcup as isize) - 3,
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            size: (scup as i32) - 2,
            tmp,
            bits: 4 - u32::from((tmp & 7) == 7),
            unstuff: (d | 0xF) > 0x8F,
        };
        r.read();
        r
    }

    /// Creates the `MagRef` reader over the refinement segment
    /// (`coded[lcup..lcup + len2]`), which is read backward and fills zeros
    /// once exhausted.
    pub fn new_mrp(coded: &'a [u8], lcup: usize, len2: usize) -> Self {
        debug_assert!(lcup + len2 <= coded.len());
        let mut r = Self {
            data: coded,
            #[allow(clippy::cast_possible_wrap)]
            pos: (lcup + len2) as isize - 1,
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            size: len2 as i32,
            tmp: 0,
            bits: 0,
            unstuff: true,
        };
        r.read();
        r
    }

    /// Reads and unstuffs up to 32 bits (backward) into the accumulator.
    fn read(&mut self) {
        if self.bits > 32 {
            return;
        }
        let mut val = 0u32;
        if self.size > 3 && self.pos >= 3 {
            let p = usize::try_from(self.pos).unwrap_or(0);
            val = u32::from_le_bytes([
                self.data[p - 3],
                self.data[p - 2],
                self.data[p - 1],
                self.data[p],
            ]);
            self.pos -= 4;
            self.size -= 4;
        } else if self.size > 0 {
            let mut i = 24u32;
            while self.size > 0 && i < 32 {
                let v = if self.pos >= 0 {
                    u32::from(self.data[usize::try_from(self.pos).unwrap_or(0)])
                } else {
                    0
                };
                self.pos -= 1;
                val |= v << i;
                self.size -= 1;
                i = i.wrapping_sub(8);
            }
        }

        // Unstuff, starting from the highest-address byte (val >> 24).
        let mut tmp = val >> 24;
        let mut bits = 8 - u32::from(self.unstuff && ((val >> 24) & 0x7F) == 0x7F);
        let mut unstuff = (val >> 24) > 0x8F;

        tmp |= ((val >> 16) & 0xFF) << bits;
        bits += 8 - u32::from(unstuff && ((val >> 16) & 0x7F) == 0x7F);
        unstuff = ((val >> 16) & 0xFF) > 0x8F;

        tmp |= ((val >> 8) & 0xFF) << bits;
        bits += 8 - u32::from(unstuff && ((val >> 8) & 0x7F) == 0x7F);
        unstuff = ((val >> 8) & 0xFF) > 0x8F;

        tmp |= (val & 0xFF) << bits;
        bits += 8 - u32::from(unstuff && (val & 0x7F) == 0x7F);
        unstuff = (val & 0xFF) > 0x8F;

        self.tmp |= u64::from(tmp) << self.bits;
        self.bits += bits;
        self.unstuff = unstuff;
    }

    /// Returns the next 32 decodable bits (little-endian order).
    pub fn fetch(&mut self) -> u32 {
        if self.bits < 32 {
            self.read();
            if self.bits < 32 {
                self.read();
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        {
            self.tmp as u32
        }
    }

    /// Consumes `num_bits` bits.
    pub fn advance(&mut self, num_bits: u32) -> u32 {
        debug_assert!(num_bits <= self.bits);
        self.tmp >>= num_bits;
        self.bits = self.bits.saturating_sub(num_bits);
        #[allow(clippy::cast_possible_truncation)]
        {
            self.tmp as u32
        }
    }
}

/// Reader for a forward-growing bitstream (`MagSgn`, `SigProp`).
///
/// Bits accumulate LSB-first; a byte following `0xFF` contributes only
/// 7 bits. `FILL` is the byte value fed once the stream is exhausted:
/// `0xFF` for `MagSgn`, `0x00` for `SigProp`.
#[derive(Debug)]
pub struct FwdReader<'a, const FILL: u8> {
    data: &'a [u8],
    pos: usize,
    size: i32,
    tmp: u64,
    bits: u32,
    unstuff: bool,
}

impl<'a, const FILL: u8> FwdReader<'a, FILL> {
    /// Creates a reader over `data`.
    pub fn new(data: &'a [u8]) -> Self {
        let mut r = Self {
            data,
            pos: 0,
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            size: data.len() as i32,
            tmp: 0,
            bits: 0,
            unstuff: false,
        };
        r.read();
        r
    }

    fn read(&mut self) {
        if self.bits > 32 {
            return;
        }
        let fill = if FILL == 0 { 0u32 } else { 0xFFFF_FFFFu32 };
        let val = if self.size > 3 {
            let v = u32::from_le_bytes([
                self.data[self.pos],
                self.data[self.pos + 1],
                self.data[self.pos + 2],
                self.data[self.pos + 3],
            ]);
            self.pos += 4;
            self.size -= 4;
            v
        } else if self.size > 0 {
            let mut v = fill;
            let mut i = 0;
            while self.size > 0 {
                let b = u32::from(self.data[self.pos]);
                self.pos += 1;
                let m = !(0xFFu32 << i);
                v = (v & m) | (b << i);
                self.size -= 1;
                i += 8;
            }
            v
        } else {
            fill
        };

        // Unstuff, starting from the lowest byte.
        let mut bits = 8 - u32::from(self.unstuff);
        let mut t = val & 0xFF;
        let mut unstuff = (val & 0xFF) == 0xFF;

        t |= ((val >> 8) & 0xFF) << bits;
        bits += 8 - u32::from(unstuff);
        unstuff = ((val >> 8) & 0xFF) == 0xFF;

        t |= ((val >> 16) & 0xFF) << bits;
        bits += 8 - u32::from(unstuff);
        unstuff = ((val >> 16) & 0xFF) == 0xFF;

        t |= ((val >> 24) & 0xFF) << bits;
        bits += 8 - u32::from(unstuff);
        self.unstuff = ((val >> 24) & 0xFF) == 0xFF;

        self.tmp |= u64::from(t) << self.bits;
        self.bits += bits;
    }

    /// Returns the next 32 decodable bits.
    pub fn fetch(&mut self) -> u32 {
        if self.bits < 32 {
            self.read();
            if self.bits < 32 {
                self.read();
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        {
            self.tmp as u32
        }
    }

    /// Consumes `num_bits` bits.
    pub fn advance(&mut self, num_bits: u32) {
        debug_assert!(num_bits <= self.bits);
        self.tmp >>= num_bits;
        self.bits = self.bits.saturating_sub(num_bits);
    }
}

/// Encoder for the VLC bitstream, which grows backward from the end of the
/// cleanup segment.
///
/// Bytes are produced in reverse order: [`Self::into_bytes`] returns them in
/// stream order (last produced first), ending with the placeholder byte that
/// later receives the high bits of `Scup`.
#[derive(Debug)]
pub struct VlcEncoder {
    /// Bytes in production order (reverse of final stream order). The first
    /// element is the `0xFF` placeholder byte.
    buf: Vec<u8>,
    /// Bit accumulator (LSB-first).
    tmp: u32,
    /// Occupied bits in [`Self::tmp`].
    used_bits: u32,
    /// True if the previously flushed byte is `> 0x8F` (stuffing state).
    last_greater_than_8f: bool,
}

impl Default for VlcEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VlcEncoder {
    /// Creates an encoder holding the initial placeholder byte.
    #[must_use]
    pub fn new() -> Self {
        let mut buf = Vec::with_capacity(2880);
        buf.push(0xFF);
        Self {
            buf,
            tmp: 0xF,
            used_bits: 4,
            last_greater_than_8f: true,
        }
    }

    /// Appends `cwd_len` bits of `cwd` (LSB-first).
    pub fn encode(&mut self, cwd: u32, cwd_len: u32) {
        let mut cwd = cwd;
        let mut cwd_len = cwd_len;
        while cwd_len > 0 {
            let mut avail_bits = 8 - u32::from(self.last_greater_than_8f) - self.used_bits;
            let t = avail_bits.min(cwd_len);
            self.tmp |= (cwd & ((1 << t) - 1)) << self.used_bits;
            self.used_bits += t;
            avail_bits -= t;
            cwd_len -= t;
            cwd >>= t;
            if avail_bits == 0 {
                if self.last_greater_than_8f && self.tmp != 0x7F {
                    // The stuffed bit is unnecessary: reclaim it.
                    self.last_greater_than_8f = false;
                    continue;
                }
                #[allow(clippy::cast_possible_truncation)]
                self.buf.push(self.tmp as u8);
                self.last_greater_than_8f = self.tmp > 0x8F;
                self.tmp = 0;
                self.used_bits = 0;
            }
        }
    }

    /// Number of bytes produced so far (including the placeholder).
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// True if only the placeholder byte exists.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.len() <= 1
    }

    /// Terminates MEL and VLC together, fusing their last bytes when possible
    /// (T.814 §7.3.5), and returns `(mel_bytes, vlc_bytes_stream_order)`.
    #[must_use]
    pub fn terminate_with_mel(mut self, mut mel: super::mel::MelEncoder) -> (Vec<u8>, Vec<u8>) {
        let (mel_tmp, mel_mask) = mel.terminate();
        let vlc_mask = if self.used_bits == 0 {
            0
        } else {
            0xFFu32 >> (8 - self.used_bits)
        };
        if (mel_mask | vlc_mask) != 0 {
            let fuse = mel_tmp | self.tmp;
            if ((fuse ^ mel_tmp) & mel_mask) | ((fuse ^ self.tmp) & vlc_mask) == 0
                && fuse != 0xFF
                && self.buf.len() > 1
            {
                #[allow(clippy::cast_possible_truncation)]
                mel.buf.push(fuse as u8);
            } else {
                #[allow(clippy::cast_possible_truncation)]
                mel.buf.push(mel_tmp as u8); // cannot be 0xFF
                #[allow(clippy::cast_possible_truncation)]
                self.buf.push(self.tmp as u8);
            }
        }
        self.buf.reverse();
        (mel.buf, self.buf)
    }
}

/// Encoder for the forward `MagSgn` bitstream (LSB-first, `0xFF`-stuffed).
#[derive(Debug)]
pub struct MagSgnEncoder {
    /// Completed bytes.
    buf: Vec<u8>,
    tmp: u32,
    used_bits: u32,
    /// 7 after an `0xFF` byte, else 8.
    max_bits: u32,
}

impl Default for MagSgnEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl MagSgnEncoder {
    /// Creates an empty encoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
            tmp: 0,
            used_bits: 0,
            max_bits: 8,
        }
    }

    /// Appends `cwd_len` bits of `cwd` (LSB-first).
    pub fn encode(&mut self, cwd: u32, cwd_len: u32) {
        let mut cwd = cwd;
        let mut cwd_len = cwd_len;
        while cwd_len > 0 {
            let t = (self.max_bits - self.used_bits).min(cwd_len);
            self.tmp |= (cwd & ((1 << t) - 1)) << self.used_bits;
            self.used_bits += t;
            cwd >>= t;
            cwd_len -= t;
            if self.used_bits >= self.max_bits {
                #[allow(clippy::cast_possible_truncation)]
                self.buf.push(self.tmp as u8);
                self.max_bits = if self.tmp == 0xFF { 7 } else { 8 };
                self.tmp = 0;
                self.used_bits = 0;
            }
        }
    }

    /// Terminates the stream (pads with ones; drops a trailing byte the
    /// decoder regenerates) and returns the bytes.
    #[must_use]
    pub fn terminate(mut self) -> Vec<u8> {
        if self.used_bits > 0 {
            let t = self.max_bits - self.used_bits;
            self.tmp |= (0xFF & ((1u32 << t) - 1)) << self.used_bits;
            self.used_bits += t;
            if self.tmp != 0xFF {
                #[allow(clippy::cast_possible_truncation)]
                self.buf.push(self.tmp as u8);
            }
        } else if self.max_bits == 7 {
            self.buf.pop();
        }
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magsgn_roundtrips_bit_patterns() {
        let words: [(u32, u32); 6] = [
            (0x5, 3),
            (0xFFFF, 16),
            (0, 7),
            (0x1234_5678, 31),
            (0x7FFF_FFFF, 31),
            (1, 1),
        ];
        let mut enc = MagSgnEncoder::new();
        for &(w, n) in &words {
            enc.encode(w & ((1 << n) - 1), n);
        }
        let bytes = enc.terminate();
        let mut dec = FwdReader::<0xFF>::new(&bytes);
        for &(w, n) in &words {
            let got = dec.fetch() & ((1 << n) - 1);
            assert_eq!(got, w & ((1 << n) - 1), "word {w:#x}/{n}");
            dec.advance(n);
        }
    }

    #[test]
    fn magsgn_stuffing_after_ff() {
        // 16 one-bits produce 0xFF then a stuffed byte with 7 usable bits.
        let mut enc = MagSgnEncoder::new();
        enc.encode(0xFFFF, 16);
        enc.encode(0, 5);
        let bytes = enc.terminate();
        assert_eq!(bytes[0], 0xFF);
        // Second byte carries bits 8..15 across 7-bit stuffing: value 0x7F
        // would violate stuffing; the encoder must keep MSB clear.
        assert!(bytes[1] <= 0x7F);
        let mut dec = FwdReader::<0xFF>::new(&bytes);
        assert_eq!(dec.fetch() & 0xFFFF, 0xFFFF);
        dec.advance(16);
        assert_eq!(dec.fetch() & 0x1F, 0);
    }

    #[test]
    fn vlc_roundtrips_backward_stream() {
        let words: [(u32, u32); 5] = [(0x3, 2), (0x7F, 7), (0x15, 5), (0x1, 3), (0x2A, 6)];
        let mut enc = VlcEncoder::new();
        for &(w, n) in &words {
            enc.encode(w, n);
        }
        let (mel_bytes, vlc_bytes) = enc.terminate_with_mel(super::super::mel::MelEncoder::new());
        // Assemble a fake cleanup segment: [MEL][VLC], then patch Scup.
        let mut seg = mel_bytes;
        seg.extend_from_slice(&vlc_bytes);
        let scup = seg.len();
        let n = seg.len();
        assert!(scup <= 0xFF0);
        #[allow(clippy::cast_possible_truncation)]
        {
            seg[n - 1] = (scup >> 4) as u8;
            seg[n - 2] = (seg[n - 2] & 0xF0) | (scup & 0xF) as u8;
        }
        let mut dec = RevReader::new_vlc(&seg, seg.len(), scup);
        for &(w, n) in &words {
            let got = dec.fetch() & ((1 << n) - 1);
            assert_eq!(got, w, "word {w:#x}/{n}");
            dec.advance(n);
        }
    }

    #[test]
    fn mrp_reader_fills_zeros() {
        let bytes = [0xABu8, 0xCD];
        let mut dec = RevReader::new_mrp(&bytes, 0, 2);
        // Stream order is backward: first bits come from 0xCD.
        assert_eq!(dec.fetch() & 0xFF, 0xCD);
        dec.advance(8);
        assert_eq!(dec.fetch() & 0xFF, 0xAB);
        dec.advance(8);
        assert_eq!(dec.fetch(), 0, "exhausted MagRef stream must feed zeros");
    }
}
