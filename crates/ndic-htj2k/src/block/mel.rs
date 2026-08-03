//! MEL (adaptive run-length) coder — T.814 §7.1.1 / §7.3.4.
//!
//! The MEL coder is a 13-state adaptation of the JPEG-LS MELCODE. The encoder
//! codes binary *events* (quad significance and initial-row u-extension
//! events); the decoder pre-decodes events into a small queue of *runs*, each
//! run being the number of zero events preceding a one event.
//!
//! Ported from `OpenJPH` `ojph_block_decoder32.cpp` (`dec_mel_st`) and
//! `ojph_block_encoder.cpp` (`mel_struct`), BSD-2-Clause.

/// MEL state exponents, indexed by the coder state `k` (T.814 Table 4).
const MEL_EXP: [u8; 13] = [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 4, 5];

/// Decoder for the MEL bitstream inside a cleanup segment.
///
/// The MEL segment starts at `lcup - scup` within the cleanup codeword
/// segment and shares its final byte with the VLC segment (the low nibble of
/// that byte belongs to the `Scup` interface word and reads as all-ones).
#[derive(Debug)]
pub struct MelDecoder<'a> {
    /// The MEL byte range (already sliced from the cleanup segment).
    data: &'a [u8],
    /// Read position within `data`.
    pos: usize,
    /// Bytes remaining (starts at `scup - 1`; the final VLC byte is excluded).
    size: i32,
    /// Bit accumulator; the next bit to decode is the MSB.
    tmp: u64,
    /// Number of valid bits in [`Self::tmp`].
    bits: i32,
    /// True if the next byte follows an `0xFF` and contributes only 7 bits.
    unstuff: bool,
    /// Coder state, 0..=12.
    k: usize,
    /// Queue of decoded runs, 7 bits each (LSB = terminated-in-one flag).
    runs: u64,
    /// Number of runs stored in [`Self::runs`].
    num_runs: u32,
}

impl<'a> MelDecoder<'a> {
    /// Creates a decoder over the MEL segment of a cleanup pass.
    ///
    /// `coded` is the whole cleanup segment (`lcup` bytes), `scup` the length
    /// of the MEL+VLC suffix.
    pub fn new(coded: &'a [u8], lcup: usize, scup: usize) -> Self {
        debug_assert!(scup >= 2 && scup <= lcup && lcup <= coded.len());
        Self {
            data: &coded[lcup - scup..],
            pos: 0,
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            // scup <= 4079 per the caller's validation.
            size: (scup - 1) as i32,
            tmp: 0,
            bits: 0,
            unstuff: false,
            k: 0,
            runs: 0,
            num_runs: 0,
        }
    }

    /// Reads and unstuffs up to 32 bits into the accumulator.
    fn read(&mut self) {
        if self.bits > 32 {
            return;
        }
        // Feed 0xFF once the buffer is exhausted (T.814 §7.3.4).
        let mut val = 0xFFFF_FFFFu32;
        if self.size > 4 {
            val = u32::from_le_bytes([
                self.data[self.pos],
                self.data[self.pos + 1],
                self.data[self.pos + 2],
                self.data[self.pos + 3],
            ]);
            self.pos += 4;
            self.size -= 4;
        } else if self.size > 0 {
            let mut i = 0;
            while self.size > 1 {
                let v = u32::from(self.data[self.pos]);
                self.pos += 1;
                let m = !(0xFFu32 << i);
                val = (val & m) | (v << i);
                self.size -= 1;
                i += 8;
            }
            // The final MEL byte is shared with the Scup interface word; its
            // low nibble must read as all-ones.
            let v = u32::from(self.data[self.pos]) | 0xF;
            self.pos += 1;
            let m = !(0xFFu32 << i);
            val = (val & m) | (v << i);
            self.size -= 1;
        }

        // Unstuff: a byte following 0xFF contributes only 7 bits.
        let mut bits = 32 - i32::from(self.unstuff);
        let mut t = val & 0xFF;
        let mut unstuff = (val & 0xFF) == 0xFF;
        bits -= i32::from(unstuff);
        t <<= 8 - u32::from(unstuff);

        t |= (val >> 8) & 0xFF;
        unstuff = ((val >> 8) & 0xFF) == 0xFF;
        bits -= i32::from(unstuff);
        t <<= 8 - u32::from(unstuff);

        t |= (val >> 16) & 0xFF;
        unstuff = ((val >> 16) & 0xFF) == 0xFF;
        bits -= i32::from(unstuff);
        t <<= 8 - u32::from(unstuff);

        t |= (val >> 24) & 0xFF;
        self.unstuff = ((val >> 24) & 0xFF) == 0xFF;

        // Left-justify so decoding consumes from the MSB of `tmp`.
        #[allow(clippy::cast_sign_loss)] // bits in 28..=32, self.bits in 0..=32
        {
            self.tmp |= u64::from(t) << (64 - bits - self.bits);
        }
        self.bits += bits;
    }

    /// Decodes MEL codewords into the run queue.
    fn decode(&mut self) {
        if self.bits < 6 {
            // 6 bits is the largest decodable MEL codeword.
            self.read();
        }
        while self.bits >= 6 && self.num_runs < 8 {
            let exp = MEL_EXP[self.k];
            let eval = u32::from(exp);
            let run: u32;
            if self.tmp & (1 << 63) != 0 {
                // A one: a complete run of 2^eval zero events, no terminator.
                run = ((1u32 << eval) - 1) << 1;
                self.k = (self.k + 1).min(12);
                self.tmp <<= 1;
                self.bits -= 1;
            } else {
                // A zero: `eval` more bits give the count of zero events
                // before a terminating one event.
                #[allow(clippy::cast_possible_truncation)]
                let count = ((self.tmp >> (63 - eval)) as u32) & ((1 << eval) - 1);
                run = (count << 1) | 1;
                self.k = self.k.saturating_sub(1);
                self.tmp <<= eval + 1;
                self.bits -= i32::from(exp) + 1;
            }
            let shift = self.num_runs * 7;
            self.runs &= !(0x3Fu64 << shift);
            self.runs |= u64::from(run) << shift;
            self.num_runs += 1;
        }
    }

    /// Retrieves the next run.
    ///
    /// The LSB of the returned value is 1 if the run terminates in a one
    /// event; the upper bits hold twice the number of zero events.
    pub fn get_run(&mut self) -> i32 {
        if self.num_runs == 0 {
            self.decode();
        }
        #[allow(clippy::cast_possible_truncation)]
        let t = (self.runs & 0x7F) as i32;
        self.runs >>= 7;
        self.num_runs -= 1;
        t
    }
}

/// Encoder for the MEL bitstream.
///
/// Bits are packed MSB-first; a byte equal to `0xFF` is followed by a
/// 7-bit byte (bit-stuffing keeps `0xFF 0x80..` sequences out of the stream).
#[derive(Debug)]
pub struct MelEncoder {
    /// Completed bytes.
    pub buf: alloc::vec::Vec<u8>,
    /// Bit accumulator (MSB-first).
    tmp: u32,
    /// Free bits remaining in [`Self::tmp`] (7 after an `0xFF` byte).
    remaining_bits: u32,
    /// Length of the current run of zero events.
    run: u32,
    /// Coder state, 0..=12.
    k: usize,
    /// `1 << MEL_EXP[k]`.
    threshold: u32,
}

impl Default for MelEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl MelEncoder {
    /// Creates an empty encoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: alloc::vec::Vec::with_capacity(192),
            tmp: 0,
            remaining_bits: 8,
            run: 0,
            k: 0,
            threshold: 1,
        }
    }

    fn emit_bit(&mut self, v: u32) {
        debug_assert!(v <= 1);
        self.tmp = (self.tmp << 1) + v;
        self.remaining_bits -= 1;
        if self.remaining_bits == 0 {
            #[allow(clippy::cast_possible_truncation)]
            self.buf.push(self.tmp as u8);
            self.remaining_bits = if self.tmp == 0xFF { 7 } else { 8 };
            self.tmp = 0;
        }
    }

    /// Encodes one binary event.
    pub fn encode(&mut self, bit: bool) {
        if bit {
            self.emit_bit(0);
            let mut t = MEL_EXP[self.k];
            while t > 0 {
                t -= 1;
                self.emit_bit((self.run >> t) & 1);
            }
            self.run = 0;
            self.k = self.k.saturating_sub(1);
            self.threshold = 1 << MEL_EXP[self.k];
        } else {
            self.run += 1;
            if self.run >= self.threshold {
                self.emit_bit(1);
                self.run = 0;
                self.k = (self.k + 1).min(12);
                self.threshold = 1 << MEL_EXP[self.k];
            }
        }
    }

    /// Flushes any incomplete run and returns `(pending_byte, mask)` for the
    /// MEL/VLC fusing step, where `mask` marks the used (high) bits.
    ///
    /// Ported from the first half of `terminate_mel_vlc`.
    pub fn terminate(&mut self) -> (u32, u32) {
        if self.run > 0 {
            self.emit_bit(1);
        }
        let tmp = self.tmp << self.remaining_bits;
        let mask = (0xFF << self.remaining_bits) & 0xFF;
        (tmp, mask)
    }
}

extern crate alloc;

#[cfg(test)]
#[allow(clippy::cast_sign_loss)] // decoded runs are non-negative by contract
mod tests {
    use super::*;

    /// Encode a sequence of events, terminate through the real MEL+VLC
    /// assembly (empty VLC stream), and decode them back.
    fn roundtrip(events: &[bool]) {
        let mut enc = MelEncoder::new();
        for &e in events {
            enc.encode(e);
        }
        let vlc = super::super::streams::VlcEncoder::new();
        let (mel_bytes, vlc_bytes) = vlc.terminate_with_mel(enc);
        let mut seg = mel_bytes;
        seg.extend_from_slice(&vlc_bytes);
        let scup = seg.len();
        let n = seg.len();
        // Patch the Scup interface word like the segment assembler does.
        #[allow(clippy::cast_possible_truncation)]
        {
            seg[n - 1] = (scup >> 4) as u8;
            seg[n - 2] = (seg[n - 2] & 0xF0) | (scup & 0xF) as u8;
        }
        let mut dec = MelDecoder::new(&seg, seg.len(), scup);

        // Reconstruct the event sequence from runs. A run field holds
        // `2 * n_zeros + 1` when it ends in a one event, else
        // `2 * (n_zeros - 1)` for a full threshold-length stretch of zeros.
        let mut decoded = alloc::vec::Vec::new();
        while decoded.len() < events.len() {
            let run = dec.get_run();
            let terminated = run & 1 == 1;
            let zeros = (run >> 1) as usize + usize::from(!terminated);
            decoded.extend(core::iter::repeat_n(false, zeros));
            if terminated {
                decoded.push(true);
            }
        }
        assert_eq!(&decoded[..events.len()], events, "events: {events:?}");
    }

    #[test]
    fn mel_roundtrips_simple_patterns() {
        roundtrip(&[true]);
        roundtrip(&[false; 40]);
        roundtrip(&[true, false, true, true, false, false, true]);
        let alternating: alloc::vec::Vec<bool> = (0..64).map(|i| i % 2 == 0).collect();
        roundtrip(&alternating);
        let sparse: alloc::vec::Vec<bool> = (0..200).map(|i| i % 17 == 0).collect();
        roundtrip(&sparse);
    }

    #[test]
    fn decoder_feeds_ones_when_exhausted() {
        // An empty MEL segment must still yield runs (from 0xFF filler).
        let seg = [0xFFu8, 0xFF];
        let mut dec = MelDecoder::new(&seg, 2, 2);
        // 0xFF filler decodes as long runs; just make sure we don't panic
        // and produce plausible runs.
        for _ in 0..16 {
            let r = dec.get_run();
            assert!((0..128).contains(&r));
        }
    }
}
