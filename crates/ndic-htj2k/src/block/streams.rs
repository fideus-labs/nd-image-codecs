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
            // One range check and one unaligned load rather than four indexed byte loads.
            val = u32::from_le_bytes(self.data[p - 3..=p].try_into().expect("4 bytes"));
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
            // One range check and one unaligned load rather than four indexed byte loads.
            let v = u32::from_le_bytes(
                self.data[self.pos..self.pos + 4]
                    .try_into()
                    .expect("4 bytes"),
            );
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

        // SWAR zero-byte test on the inverted word flags an 0xFF in the first three bytes. With
        // none, and no stuffing carried in, every byte keeps all 8 bits and the word passes as is.
        let inv = !val | 0xFF00_0000;
        if !self.unstuff && inv.wrapping_sub(0x0101_0101) & val & 0x0080_8080 == 0 {
            self.unstuff = val >> 24 == 0xFF;
            self.tmp |= u64::from(val) << self.bits;
            self.bits += 32;
            return;
        }

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

/// Unstuffs a whole `MagSgn` segment into a plain LSB-first bit stream, the same bits a
/// [`FwdReader`] with `FILL = 0xFF` yields. Each byte lands whole at a running bit position that
/// advances by 8, or by 7 after an `0xFF`, so a stuffed byte's MSB overlaps the next byte's
/// first bit. Past the segment every bit reads as one, and the output ends with padding for
/// 8-byte loads. The output replaces `out`'s contents, reusing its capacity.
pub(crate) fn unstuff_magsgn(data: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(data.len() + 24);
    // Pending output bits below `nbits`, plus possibly the overlapping MSB of a 7-bit byte at
    // `nbits`.
    let mut acc = 0u64;
    let mut nbits = 0u32;
    let mut after_ff = false;
    let (chunks, tail) = data.as_chunks::<8>();
    for &chunk in chunks {
        let w = u64::from_le_bytes(chunk);
        // A SWAR zero-byte test on the inverted word flags an 0xFF in the first seven bytes.
        // With none, and none carried in, all eight bytes keep 8 bits and append as one word.
        let ff = (!w).wrapping_sub(0x0101_0101_0101_0101) & w & 0x0080_8080_8080_8080;
        if !after_ff && ff == 0 {
            out.extend_from_slice(&(acc | (w << nbits)).to_le_bytes());
            // The bits that did not fit; `(w >> 1) >> (63 - nbits)` is zero when `nbits` is 0.
            acc = (w >> 1) >> (63 - nbits);
            after_ff = w >> 56 == 0xFF;
        } else {
            for b in chunk {
                push_byte(out, &mut acc, &mut nbits, &mut after_ff, b);
            }
        }
    }
    for &b in tail {
        push_byte(out, &mut acc, &mut nbits, &mut after_ff, b);
    }
    // The reader feeds 0xFF past the segment, so every later bit is one.
    out.extend_from_slice(&(acc | (u64::MAX << nbits)).to_le_bytes());
    out.extend_from_slice(&[0xFF; 16]);
}

/// Places one byte of [`unstuff_magsgn`] whole at the running position, which advances 8 bits,
/// or 7 after an `0xFF`.
fn push_byte(out: &mut Vec<u8>, acc: &mut u64, nbits: &mut u32, after_ff: &mut bool, b: u8) {
    let wide = u128::from(*acc) | (u128::from(b) << *nbits);
    *nbits += 8 - u32::from(*after_ff);
    *after_ff = b == 0xFF;
    if *nbits >= 64 {
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(wide as u64).to_le_bytes());
        *acc = (wide >> 64) as u64;
        *nbits -= 64;
    } else {
        #[allow(clippy::cast_possible_truncation)] // every placed bit is below 64
        {
            *acc = wide as u64;
        }
    }
}

/// Unstuffs a cleanup segment's VLC stream into a plain LSB-first bit stream, the same bits
/// [`RevReader::new_vlc`] yields. The stream starts with the high nibble of `coded[lcup - 2]`,
/// then takes bytes backward from `coded[lcup - 3]` to `coded[lcup - scup]`. Each lands whole at
/// a running bit position that advances by 8, or by 7 when the byte before was above `0x8F` and
/// this one's low 7 bits are all ones. Past the stream every bit reads as zero, and the output
/// replaces `out`'s contents, reusing its capacity.
pub(crate) fn unstuff_vlc(coded: &[u8], lcup: usize, scup: usize, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(scup + 24);
    let d = coded[lcup - 2];
    // The interface byte's high nibble keeps 3 bits when its low three are all ones.
    let mut acc = u64::from(d >> 4);
    let mut nbits = 4 - u32::from((d >> 4) & 7 == 7);
    let mut after_8f = (d | 0xF) > 0x8F;
    let bytes = &coded[lcup - scup..lcup - 2];
    let (tail, chunks) = bytes.as_rchunks::<8>();
    for &chunk in chunks.iter().rev() {
        // The byte read first, the highest address, lands lowest.
        let w = u64::from_be_bytes(chunk);
        // A SWAR zero-byte test flags a byte whose low 7 bits are all ones, the only kind that
        // can be stuffed. With none, all eight bytes keep 8 bits and append as one word.
        if (!w & 0x7F7F_7F7F_7F7F_7F7F).wrapping_sub(0x0101_0101_0101_0101) & 0x8080_8080_8080_8080
            == 0
        {
            out.extend_from_slice(&(acc | (w << nbits)).to_le_bytes());
            // The bits that did not fit; `(w >> 1) >> (63 - nbits)` is zero when `nbits` is 0.
            acc = (w >> 1) >> (63 - nbits);
            after_8f = w >> 56 > 0x8F;
        } else {
            for b in chunk.into_iter().rev() {
                push_vlc_byte(out, &mut acc, &mut nbits, &mut after_8f, b);
            }
        }
    }
    for &b in tail.iter().rev() {
        push_vlc_byte(out, &mut acc, &mut nbits, &mut after_8f, b);
    }
    // The reader feeds zeros past the stream.
    out.extend_from_slice(&acc.to_le_bytes());
    out.extend_from_slice(&[0; 16]);
}

/// Places one byte of [`unstuff_vlc`] whole at the running position, which advances 8 bits, or
/// 7 for a stuffed byte.
fn push_vlc_byte(out: &mut Vec<u8>, acc: &mut u64, nbits: &mut u32, after_8f: &mut bool, b: u8) {
    let wide = u128::from(*acc) | (u128::from(b) << *nbits);
    *nbits += 8 - u32::from(*after_8f && b & 0x7F == 0x7F);
    *after_8f = b > 0x8F;
    if *nbits >= 64 {
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(wide as u64).to_le_bytes());
        *acc = (wide >> 64) as u64;
        *nbits -= 64;
    } else {
        #[allow(clippy::cast_possible_truncation)] // every placed bit is below 64
        {
            *acc = wide as u64;
        }
    }
}

/// An LSB-first reader over a stream from [`unstuff_vlc`], small enough to live in registers.
/// Bits past the end read as zero.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CleanBits {
    /// Bit buffer, next bit lowest.
    buf: u64,
    /// Valid bits in [`Self::buf`].
    cnt: u32,
    /// Next byte of the stream not yet wholly in the buffer.
    pos: usize,
}

impl CleanBits {
    /// Tops the buffer up to at least 56 bits and returns the low 32. The next eight bytes are
    /// merged in at the fill level, then the position advances past the whole bytes that fit, so
    /// the refill needs no branch (the unconditional refill zlib-ng's inflate uses).
    #[inline]
    pub(crate) fn fetch(&mut self, clean: &[u8]) -> u32 {
        // The stream ends in more than eight zero bytes of padding, so a word that runs off its
        // end holds only zeros. Taking that constant keeps a copy call out of the decode loop.
        let word = clean
            .get(self.pos..)
            .and_then(<[u8]>::first_chunk::<8>)
            .map_or(0, |w| u64::from_le_bytes(*w));
        self.buf |= word << self.cnt;
        self.pos += ((63 - self.cnt) >> 3) as usize;
        // The count is now at least 56, and adding the whole bytes taken is setting those bits.
        self.cnt |= 0b11_1000;
        #[allow(clippy::cast_possible_truncation)]
        {
            self.buf as u32
        }
    }

    /// Consumes `n` bits and returns the next 32.
    #[inline]
    pub(crate) fn advance(&mut self, n: u32) -> u32 {
        debug_assert!(n <= self.cnt);
        self.buf >>= n;
        self.cnt -= n;
        #[allow(clippy::cast_possible_truncation)]
        {
            self.buf as u32
        }
    }
}

/// Encoder for the VLC bitstream, which grows backward from the end of the
/// cleanup segment.
///
/// Bytes are produced in reverse order: [`Self::into_bytes`] returns them in
/// stream order (last produced first), ending with the placeholder byte that
/// later receives the high bits of `Scup`.
///
/// Codewords go in with [`Self::merge`] and complete bytes leave with [`Self::flush`], a
/// byte-granular writer after libdeflate and igzip: the flush stores all eight accumulator
/// bytes unconditionally and advances past the complete ones, so how many bits the codewords
/// brought never steers a branch, and several codewords share one flush.
#[derive(Debug)]
pub struct VlcEncoder {
    /// Bytes in production order (reverse of final stream order) in `buf[..len]`. The first
    /// is the `0xFF` placeholder byte. Bytes past `len` are scratch that the flush's 8-byte
    /// store runs into.
    buf: Vec<u8>,
    /// Number of bytes produced.
    len: usize,
    /// Bit accumulator (LSB-first). Fewer than 8 bits wait here after a flush.
    acc: u64,
    /// Occupied bits in [`Self::acc`].
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
        Self::with_buffer(Vec::new())
    }

    /// Creates an encoder holding the initial placeholder byte that writes into `buf`, reusing
    /// its capacity. Its old bytes stay as scratch past the logical end rather than being
    /// cleared and zero-filled again.
    #[must_use]
    pub fn with_buffer(mut buf: Vec<u8>) -> Self {
        if buf.is_empty() {
            buf.push(0);
        }
        buf[0] = 0xFF;
        Self {
            buf,
            len: 1,
            acc: 0xF,
            used_bits: 4,
            last_greater_than_8f: true,
        }
    }

    /// Appends `cwd_len` bits of `cwd` (LSB-first).
    pub fn encode(&mut self, cwd: u32, cwd_len: u32) {
        self.encode_all(&[(cwd, cwd_len)]);
    }

    /// Appends each `(cwd, cwd_len)` codeword in order, as repeated
    /// [`Self::encode`] calls would.
    pub fn encode_all(&mut self, words: &[(u32, u32)]) {
        for &(cwd, cwd_len) in words {
            self.merge(u64::from(cwd) & ((1u64 << cwd_len.min(32)) - 1), cwd_len);
            self.flush();
        }
    }

    /// Appends the `cwd_len`-bit codeword `cwd`, which holds no bits above them, without
    /// writing any bytes. The caller flushes before the pending bits could pass 63.
    #[inline]
    pub(crate) fn merge(&mut self, cwd: u64, cwd_len: u32) {
        debug_assert!(cwd_len <= 32 && cwd >> cwd_len == 0);
        debug_assert!(self.used_bits + cwd_len <= 63);
        self.acc |= cwd << self.used_bits;
        self.used_bits += cwd_len;
    }

    /// Writes out the complete bytes, leaving fewer than 8 bits pending.
    #[inline]
    pub(crate) fn flush(&mut self) {
        // Bits in complete bytes.
        let done = self.used_bits & !7;
        let acc = self.acc;
        // A SWAR zero-byte test flags bytes whose low 7 bits are all ones, the only kind that
        // can be stuffed. One among the complete bytes sends them out a byte at a time.
        let sevens = (!acc & 0x7F7F_7F7F_7F7F_7F7F).wrapping_sub(0x0101_0101_0101_0101)
            & 0x8080_8080_8080_8080;
        if sevens & (1u64 << done).wrapping_sub(1) != 0 {
            self.flush_bytes();
            return;
        }
        let pos = self.len;
        if self.buf.len() < pos + 8 {
            self.buf.resize((pos + 8).max(2 * self.buf.len()), 0);
        }
        self.buf[pos..pos + 8].copy_from_slice(&acc.to_le_bytes());
        self.len = pos + (done / 8) as usize;
        let last = (acc >> (done.wrapping_sub(8) & 63)) & 0xFF;
        self.last_greater_than_8f = if done == 0 {
            self.last_greater_than_8f
        } else {
            last > 0x8F
        };
        self.acc = acc >> done;
        self.used_bits -= done;
    }

    /// Writes out the complete bytes one at a time, applying the stuffing rule to each.
    #[cold]
    fn flush_bytes(&mut self) {
        while let Some(byte) = Self::take_byte(
            &mut self.acc,
            &mut self.used_bits,
            &mut self.last_greater_than_8f,
        ) {
            if self.buf.len() > self.len {
                self.buf[self.len] = byte;
            } else {
                self.buf.push(byte);
            }
            self.len += 1;
        }
    }

    /// Takes the next byte out of the accumulator if it is complete. After a
    /// byte above `0x8F` the next one keeps its MSB stuffed only when its low
    /// 7 bits are `0x7F`. Otherwise the stuffed bit is unnecessary and is
    /// reclaimed.
    fn take_byte(
        acc: &mut u64,
        used_bits: &mut u32,
        last_greater_than_8f: &mut bool,
    ) -> Option<u8> {
        let n = 8 - u32::from(*last_greater_than_8f && *acc & 0x7F == 0x7F);
        if *used_bits < n {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let byte = (*acc & ((1 << n) - 1)) as u8;
        *acc >>= n;
        *used_bits -= n;
        *last_greater_than_8f = byte > 0x8F;
        Some(byte)
    }

    /// Number of bytes produced so far (including the placeholder).
    #[must_use]
    pub fn len(&self) -> usize {
        // Completed bytes may still wait in the accumulator, so they are counted too.
        let mut acc = self.acc;
        let (mut used_bits, mut last) = (self.used_bits, self.last_greater_than_8f);
        let mut n = self.len;
        while Self::take_byte(&mut acc, &mut used_bits, &mut last).is_some() {
            n += 1;
        }
        n
    }

    /// True if only the placeholder byte exists.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() <= 1
    }

    /// Terminates MEL and VLC together, fusing their last bytes when possible
    /// (T.814 §7.3.5), and returns `(mel_bytes, vlc_bytes_stream_order)`.
    #[must_use]
    pub fn terminate_with_mel(mut self, mut mel: super::mel::MelEncoder) -> (Vec<u8>, Vec<u8>) {
        self.buf.truncate(self.len);
        // Flush the completed bytes still waiting in the accumulator.
        self.flush_bytes();
        #[allow(clippy::cast_possible_truncation)] // fewer than 8 bits remain
        let vlc_tmp = self.acc as u32;
        let (mel_tmp, mel_mask) = mel.terminate();
        let vlc_mask = if self.used_bits == 0 {
            0
        } else {
            0xFFu32 >> (8 - self.used_bits)
        };
        if (mel_mask | vlc_mask) != 0 {
            let fuse = mel_tmp | vlc_tmp;
            if ((fuse ^ mel_tmp) & mel_mask) | ((fuse ^ vlc_tmp) & vlc_mask) == 0
                && fuse != 0xFF
                && self.buf.len() > 1
            {
                #[allow(clippy::cast_possible_truncation)]
                mel.buf.push(fuse as u8);
            } else {
                #[allow(clippy::cast_possible_truncation)]
                mel.buf.push(mel_tmp as u8); // cannot be 0xFF
                #[allow(clippy::cast_possible_truncation)]
                self.buf.push(vlc_tmp as u8);
            }
        }
        self.buf.reverse();
        (mel.buf, self.buf)
    }
}

/// Encoder for the forward `MagSgn` bitstream (LSB-first, `0xFF`-stuffed), with the same
/// merge and flush split as [`VlcEncoder`].
#[derive(Debug)]
pub struct MagSgnEncoder {
    /// Completed bytes in `buf[..len]`. Bytes past `len` are scratch that the flush's 8-byte
    /// store runs into.
    buf: Vec<u8>,
    /// Number of completed bytes.
    len: usize,
    /// Pending bits, LSB-first. Fewer than 8 wait here after a flush.
    acc: u64,
    /// Occupied bits in [`Self::acc`].
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
        Self::with_buffer(Vec::new())
    }

    /// Creates an empty encoder that writes into `buf`, reusing its capacity. Its old bytes stay
    /// as scratch past the logical end rather than being cleared and zero-filled again.
    #[must_use]
    pub fn with_buffer(buf: Vec<u8>) -> Self {
        Self {
            buf,
            len: 0,
            acc: 0,
            used_bits: 0,
            max_bits: 8,
        }
    }

    /// Appends `cwd_len` bits of `cwd` (LSB-first).
    pub fn encode(&mut self, cwd: u32, cwd_len: u32) {
        self.encode_all(&[(cwd, cwd_len)]);
    }

    /// Appends each `(cwd, cwd_len)` codeword in order, as repeated
    /// [`Self::encode`] calls would.
    pub fn encode_all(&mut self, words: &[(u32, u32)]) {
        for &(cwd, cwd_len) in words {
            self.merge(u64::from(cwd) & ((1u64 << cwd_len.min(32)) - 1), cwd_len);
            self.flush();
        }
    }

    /// Appends the `cwd_len`-bit codeword `cwd`, which holds no bits above them, without
    /// writing any bytes. The caller flushes before the pending bits could pass 63.
    #[inline]
    pub(crate) fn merge(&mut self, cwd: u64, cwd_len: u32) {
        debug_assert!(cwd_len <= 32 && cwd >> cwd_len == 0);
        debug_assert!(self.used_bits + cwd_len <= 63);
        self.acc |= cwd << self.used_bits;
        self.used_bits += cwd_len;
    }

    /// Writes out the complete bytes, leaving fewer than 8 bits pending.
    #[inline(always)]
    #[allow(clippy::inline_always)] // every quad of the cleanup loop flushes
    pub(crate) fn flush(&mut self) {
        // Bits in complete bytes.
        let done = self.used_bits & !7;
        let acc = self.acc;
        // A SWAR zero-byte test on the inverted accumulator flags 0xFF bytes. One before the
        // last complete byte, or a stuffed byte pending from before, sends the bytes out one at
        // a time, since each 0xFF shortens the byte after it to 7 bits.
        let ff = (!acc).wrapping_sub(0x0101_0101_0101_0101) & acc & 0x8080_8080_8080_8080;
        if self.max_bits != 8 || ff & ((1u64 << done).wrapping_sub(1) >> 8) != 0 {
            self.flush_bytes();
            return;
        }
        let pos = self.len;
        if self.buf.len() < pos + 8 {
            self.buf.resize((pos + 8).max(2 * self.buf.len()), 0);
        }
        self.buf[pos..pos + 8].copy_from_slice(&acc.to_le_bytes());
        self.len = pos + (done / 8) as usize;
        // A last complete byte of 0xFF shortens the next one. With no 0xFF below it, its flag is
        // exact.
        let last_ff = done != 0 && (ff >> (done.wrapping_sub(1) & 63)) & 1 != 0;
        self.max_bits = 8 - u32::from(last_ff);
        self.acc = acc >> done;
        self.used_bits -= done;
    }

    /// Writes out the complete bytes one at a time, applying the stuffing rule to each.
    #[cold]
    fn flush_bytes(&mut self) {
        while self.used_bits >= self.max_bits {
            let byte = self.acc & ((1 << self.max_bits) - 1);
            #[allow(clippy::cast_possible_truncation)]
            let b = byte as u8;
            if self.buf.len() > self.len {
                self.buf[self.len] = b;
            } else {
                self.buf.push(b);
            }
            self.len += 1;
            self.acc >>= self.max_bits;
            self.used_bits -= self.max_bits;
            self.max_bits = if byte == 0xFF { 7 } else { 8 };
        }
    }

    /// Terminates the stream (pads with ones; drops a trailing byte the
    /// decoder regenerates) and returns the bytes.
    #[must_use]
    pub fn terminate(mut self) -> Vec<u8> {
        self.buf.truncate(self.len);
        // Flush the completed bytes still waiting in the accumulator.
        self.flush_bytes();
        #[allow(clippy::cast_possible_truncation)] // fewer than 8 bits remain
        let mut tmp = self.acc as u32;
        if self.used_bits > 0 {
            let t = self.max_bits - self.used_bits;
            tmp |= (0xFF & ((1u32 << t) - 1)) << self.used_bits;
            if tmp != 0xFF {
                #[allow(clippy::cast_possible_truncation)]
                self.buf.push(tmp as u8);
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
    fn unstuff_vlc_matches_the_reader() {
        // Heavy in bytes above 0x8F and bytes whose low 7 bits are all ones, so stuffing lands in
        // every chunk position, in both paths, and against the interface nibble.
        let mut state = 0x9E37_79B9u32;
        for scup in [2usize, 3, 5, 9, 10, 11, 17, 18, 64, 65, 200, 1000] {
            for mix in [2u32, 3, 7, 1000] {
                let lcup = scup + 5;
                let coded: Vec<u8> = (0..lcup)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        let r = (state >> 24) as u8;
                        match (state >> 8) % mix {
                            0 => r | 0x7F,
                            1 => r | 0x90,
                            _ => r,
                        }
                    })
                    .collect();
                let mut clean = Vec::new();
                unstuff_vlc(&coded, lcup, scup, &mut clean);
                let mut reader = RevReader::new_vlc(&coded, lcup, scup);
                let mut bits = CleanBits::default();
                for i in 0..(clean.len() / 4) {
                    let want = reader.fetch();
                    reader.advance(32);
                    assert_eq!(bits.fetch(&clean), want, "scup {scup} mix {mix} word {i}");
                    bits.advance(32);
                }
            }
        }
    }

    #[test]
    fn unstuff_magsgn_matches_the_reader() {
        // Heavy in 0xFF so stuffing lands in every chunk position, in both paths, and at the end.
        let mut state = 0x2545_F491u32;
        for len in [0usize, 1, 7, 8, 9, 15, 16, 17, 63, 64, 65, 200, 1000] {
            for ff_every in [2u32, 5, 17, 1000] {
                let data: Vec<u8> = (0..len)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        if (state >> 8).is_multiple_of(ff_every) {
                            0xFF
                        } else {
                            (state >> 24) as u8
                        }
                    })
                    .collect();
                let mut clean = Vec::new();
                unstuff_magsgn(&data, &mut clean);
                let mut reader = FwdReader::<0xFF>::new(&data);
                for (i, word) in clean.as_chunks::<4>().0.iter().enumerate() {
                    let want = reader.fetch();
                    reader.advance(32);
                    assert_eq!(
                        u32::from_le_bytes(*word),
                        want,
                        "len {len} ff {ff_every} word {i}"
                    );
                }
            }
        }
    }

    #[test]
    fn vlc_len_counts_bytes_waiting_in_the_accumulator() {
        let mut state = 0x1234_5678u32;
        let words: Vec<(u32, u32)> = (0..200)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 3, state >> 29)
            })
            .collect();
        // For every prefix, terminating adds at most the one partial byte, so nothing completed
        // went uncounted by `len`.
        for k in 0..=words.len() {
            let mut enc = VlcEncoder::new();
            enc.encode_all(&words[..k]);
            let len = enc.len();
            let (_, bytes) = enc.terminate_with_mel(super::super::mel::MelEncoder::new());
            assert!((len..=len + 1).contains(&bytes.len()), "prefix {k}");
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
