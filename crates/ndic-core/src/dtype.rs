//! Sample dtype descriptor.

/// The pixel/voxel sample types nd-image-codecs can code.
///
/// HTJ2K (like JPEG 2000 Part 1) codes integer wavelet coefficients; the
/// reversible 5/3 path is bit-exact for `U8`/`I8`/`U16`/`I16`, and the
/// irreversible 9/7 path targets the same integer sample grid with a lossy
/// quantizer. Bit depth up to 16 covers the overwhelming majority of microscopy
/// (8-bit RGB, 16-bit monochrome) and OME-Zarr microscopy (`uint8`/`uint16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SampleType {
    /// Unsigned 8-bit (e.g. RGB whole-slide-image channels).
    U8,
    /// Signed 8-bit.
    I8,
    /// Unsigned 16-bit (e.g. 16-bit monochrome pathology / fluorescence).
    U16,
    /// Signed 16-bit (e.g. CT Hounsfield units).
    I16,
    /// Unsigned 32-bit (label maps; coded losslessly only).
    U32,
    /// Signed 32-bit.
    I32,
}

impl SampleType {
    /// Bytes per sample in the packed little-endian representation.
    #[must_use]
    pub const fn size_bytes(self) -> usize {
        match self {
            SampleType::U8 | SampleType::I8 => 1,
            SampleType::U16 | SampleType::I16 => 2,
            SampleType::U32 | SampleType::I32 => 4,
        }
    }

    /// Nominal bit depth carried in the `SIZ` marker `Ssiz` field.
    #[must_use]
    pub const fn bit_depth(self) -> u8 {
        match self {
            SampleType::U8 | SampleType::I8 => 8,
            SampleType::U16 | SampleType::I16 => 16,
            SampleType::U32 | SampleType::I32 => 32,
        }
    }

    /// Whether the type is signed (drives the `Ssiz` sign bit).
    #[must_use]
    pub const fn is_signed(self) -> bool {
        matches!(self, SampleType::I8 | SampleType::I16 | SampleType::I32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_depth_agree() {
        assert_eq!(SampleType::U16.size_bytes(), 2);
        assert_eq!(SampleType::U16.bit_depth(), 16);
        assert!(!SampleType::U16.is_signed());
        assert!(SampleType::I16.is_signed());
    }
}
