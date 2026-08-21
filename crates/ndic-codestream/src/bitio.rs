//! Packet-header bit I/O with JPEG 2000 bit stuffing (T.800 §B.10.1).
//!
//! Bits are packed MSB-first. After a byte equal to `0xFF`, the next byte
//! carries only 7 bits (its MSB is a stuffed 0), so `0xFF 0x80..0xFF` byte
//! pairs never appear inside a packet header. A header never ends with a
//! raw `0xFF`: the terminating flush emits the stuffed byte.

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{Error, Result};

/// Bit writer for packet headers.
#[derive(Debug)]
pub struct HeaderBitWriter {
    /// Completed bytes.
    pub out: Vec<u8>,
    tmp: u32,
    avail: u32,
}

impl Default for HeaderBitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl HeaderBitWriter {
    /// Creates an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            tmp: 0,
            avail: 8,
        }
    }

    /// Appends one bit.
    pub fn put_bit(&mut self, bit: u32) {
        self.avail -= 1;
        self.tmp |= (bit & 1) << self.avail;
        if self.avail == 0 {
            #[allow(clippy::cast_possible_truncation)]
            self.out.push(self.tmp as u8);
            self.avail = if self.tmp == 0xFF { 7 } else { 8 };
            self.tmp = 0;
        }
    }

    /// Appends `num_bits` bits of `data`, MSB first.
    pub fn put_bits(&mut self, data: u32, num_bits: u32) {
        debug_assert!(num_bits <= 32);
        for i in (0..num_bits).rev() {
            self.put_bit(data >> i);
        }
    }

    /// Appends `n` zero bits.
    pub fn put_zeros(&mut self, n: u32) {
        for _ in 0..n {
            self.put_bit(0);
        }
    }

    /// Flushes the partial byte (including the stuffed byte after a trailing
    /// `0xFF`) and returns the header bytes.
    #[must_use]
    pub fn terminate(mut self) -> Vec<u8> {
        if self.avail < 8 {
            #[allow(clippy::cast_possible_truncation)]
            self.out.push(self.tmp as u8);
        }
        self.out
    }
}

/// Bit reader for packet headers over an in-memory byte range.
#[derive(Debug)]
pub struct HeaderBitReader<'a> {
    data: &'a [u8],
    /// Next read position within `data` (the sub-slice being read, not the
    /// parent it was taken from).
    pub pos: usize,
    /// Offset of `data` within the parent slice, from `subslice_range`.
    start: usize,
    /// Offset added to `pos` in reported errors (`base + start` of the
    /// constructing call).
    base: usize,
    tmp: u32,
    avail: u32,
    unstuff: bool,
}

impl<'a> HeaderBitReader<'a> {
    /// Creates a reader over `data`.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self::new_in(data, data, 0)
    }

    /// Creates a reader over `sub`, a sub-slice of `parent` whose own first
    /// byte sits at `base` in the caller's coordinate space.
    ///
    /// Where `sub` begins inside `parent` is recovered from the two slices
    /// with [`slice::subslice_range`] rather than passed alongside them, so
    /// the offsets this reader reports — and the header length
    /// [`HeaderBitReader::terminate`] returns, which is measured in
    /// `parent` — are anchored to the bytes actually being read. A caller
    /// can no longer hand over a slice and an offset that disagree about it.
    ///
    /// `sub` not being part of `parent` is a caller bug, not malformed
    /// input, so it degrades to an offset of zero (reported error positions
    /// only) rather than panicking: this parser runs on hostile bytes and
    /// never panics on them.
    #[must_use]
    pub fn new_in(parent: &'a [u8], sub: &'a [u8], base: usize) -> Self {
        let range = parent.subslice_range(sub);
        debug_assert!(range.is_some(), "sub must be a sub-slice of parent");
        let start = range.map_or(0, |r| r.start);
        Self {
            data: sub,
            pos: 0,
            start,
            base: base + start,
            tmp: 0,
            avail: 0,
            unstuff: false,
        }
    }

    fn fill(&mut self) -> Result<()> {
        let Some(&t) = self.data.get(self.pos) else {
            return Err(Error::Codestream {
                offset: self.base + self.pos,
                message: "packet header truncated".into(),
            });
        };
        self.pos += 1;
        self.tmp = u32::from(t);
        self.avail = 8 - u32::from(self.unstuff);
        self.unstuff = t == 0xFF;
        Ok(())
    }

    /// Reads one bit.
    ///
    /// # Errors
    /// [`Error::Codestream`] when the header is truncated.
    pub fn read_bit(&mut self) -> Result<u32> {
        if self.avail == 0 {
            self.fill()?;
        }
        self.avail -= 1;
        Ok((self.tmp >> self.avail) & 1)
    }

    /// Reads `num_bits` bits, MSB first.
    ///
    /// # Errors
    /// [`Error::Codestream`] when the header is truncated.
    pub fn read_bits(&mut self, num_bits: u32) -> Result<u32> {
        debug_assert!(num_bits <= 32);
        let mut v = 0;
        for _ in 0..num_bits {
            v = (v << 1) | self.read_bit()?;
        }
        Ok(v)
    }

    /// Finishes the header: aligns to a byte boundary, consuming the stuffed
    /// byte after a trailing `0xFF`, and returns the total header length
    /// **measured in the parent slice** — for a reader built by
    /// [`HeaderBitReader::new`] the two coincide.
    pub fn terminate(mut self) -> usize {
        if self.unstuff {
            // The next byte is the stuffed one; it belongs to the header.
            self.pos += usize::from(self.pos < self.data.len());
        }
        self.start + self.pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_with_stuffing() {
        let mut w = HeaderBitWriter::new();
        // Force an 0xFF byte then more bits.
        w.put_bits(0xFF, 8);
        w.put_bits(0b1011, 4);
        w.put_bit(1);
        let bytes = w.terminate();
        assert_eq!(bytes[0], 0xFF);
        assert!(bytes[1] < 0x80, "stuffed bit must clear the MSB");

        let mut r = HeaderBitReader::new(&bytes);
        assert_eq!(r.read_bits(8).unwrap(), 0xFF);
        assert_eq!(r.read_bits(4).unwrap(), 0b1011);
        assert_eq!(r.read_bit().unwrap(), 1);
    }

    #[test]
    fn trailing_ff_gets_stuffed_byte() {
        let mut w = HeaderBitWriter::new();
        w.put_bits(0xFF, 8);
        let bytes = w.terminate();
        // avail was 7 after the 0xFF flush -> terminate writes the pad byte.
        assert_eq!(bytes, alloc::vec![0xFF, 0x00]);
    }

    #[test]
    fn truncated_header_errors() {
        let mut r = HeaderBitReader::new(&[]);
        assert!(r.read_bit().is_err());
    }

    #[test]
    fn new_in_recovers_the_sub_slice_offset() {
        // A header sitting behind a six-byte SOP segment. The reader is
        // handed the parent and the sub-slice; nobody adds 6 by hand.
        let mut w = HeaderBitWriter::new();
        w.put_bits(0b1011, 4);
        let header = w.terminate();
        let mut data = alloc::vec![0xFF, 0x91, 0x00, 0x04, 0x00, 0x00];
        data.extend_from_slice(&header);

        let mut r = HeaderBitReader::new_in(&data, &data[6..], 1000);
        assert_eq!(r.read_bits(4).unwrap(), 0b1011);
        // terminate() measures in the parent, so it spans the SOP bytes too.
        assert_eq!(r.terminate(), 6 + header.len());

        // Reported offsets are the caller's base plus the position in the
        // parent — the sub-slice's own start included.
        let mut r = HeaderBitReader::new_in(&data, &data[data.len()..], 1000);
        let Err(Error::Codestream { offset, .. }) = r.read_bit() else {
            panic!("a truncated header must error");
        };
        assert_eq!(offset, 1000 + data.len());

        // `new` is the degenerate case: the parent is the sub-slice.
        assert_eq!(HeaderBitReader::new(&data).terminate(), 0);
    }
}
