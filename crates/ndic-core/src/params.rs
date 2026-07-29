//! Encode-side parameter structs describing a codestream configuration.

/// Progression order — the order in which packets are laid out in the
/// codestream. nd-image-codecs defaults to **RPCL** because it front-loads
/// whole low-resolution levels, so a single HTTP Range request can pull a
/// thumbnail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressionOrder {
    /// Layer / Resolution / Component / Position.
    Lrcp,
    /// Resolution / Layer / Component / Position.
    Rlcp,
    /// Resolution / Position / Component / Layer — the nd-image-codecs default.
    #[default]
    Rpcl,
    /// Position / Component / Resolution / Layer.
    Pcrl,
    /// Component / Position / Resolution / Layer.
    Cprl,
}

/// Which wavelet kernel to use for the in-plane (x/y) 2D transform. (Cross-
/// axis decorrelation is configured on the `nd_lift` codec, not here.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaveletKind {
    /// Reversible Le Gall 5/3 — bit-exact/lossless.
    #[default]
    Reversible53,
    /// Irreversible Cohen–Daubechies–Feauveau 9/7 — lossy, higher ratio.
    Irreversible97,
}

/// Code-block coding-style flags (`SPcod`/`SPcoc` `cbstyle` bits). HT-specific
/// behaviour is signaled separately via the `CAP`/`Scap` marker; these are the
/// classic Part-1 toggles nd-image-codecs honours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CodeBlockStyle {
    /// Use vertically-causal context (bit 3).
    pub vertically_causal: bool,
    /// Reset context probabilities on each pass (bit 1) — HT ignores.
    pub reset_on_pass: bool,
}

/// The full set of knobs the 2D plane encoder reads. Sensible OME-Zarr-
/// friendly defaults are provided by [`EncodeParams::default`].
#[derive(Debug, Clone, PartialEq)]
pub struct EncodeParams {
    /// Number of wavelet decomposition levels in x/y (resolutions = this + 1).
    pub xy_levels: u8,
    /// Code-block dimensions (power of two, 4..=1024, product ≤ 4096) — HT uses
    /// 64×64 by default, matching `OpenJPH`.
    pub code_block: (u16, u16),
    /// Precinct sizes per resolution (`PPx`/`PPy`); empty ⇒ maximal precincts.
    pub precincts: AllocVec<(u16, u16)>,
    /// Progression order.
    pub progression: ProgressionOrder,
    /// In-plane and inter-slice wavelet kernel.
    pub wavelet: WaveletKind,
    /// Number of quality layers.
    pub quality_layers: u16,
    /// Emit `TLM` (main-header tile-part lengths) and `PLT` (tile-part packet
    /// lengths) marker segments so a reader can index the codestream and issue
    /// byte-range reads without decoding.
    pub emit_tlm_plt: bool,
    /// Code-block style toggles.
    pub cbstyle: CodeBlockStyle,
}

impl Default for EncodeParams {
    /// Streaming-friendly defaults: RPCL, 5 xy levels, 64×64 HT code-blocks,
    /// reversible 5/3, and indexing markers on so byte-range thumbnail access
    /// works out of the box.
    fn default() -> Self {
        Self {
            xy_levels: 5,
            code_block: (64, 64),
            precincts: AllocVec::new(),
            progression: ProgressionOrder::Rpcl,
            wavelet: WaveletKind::Reversible53,
            quality_layers: 1,
            emit_tlm_plt: true,
            cbstyle: CodeBlockStyle::default(),
        }
    }
}

#[cfg(feature = "std")]
type AllocVec<T> = std::vec::Vec<T>;
#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(not(feature = "std"))]
type AllocVec<T> = alloc::vec::Vec<T>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_streaming_friendly() {
        let p = EncodeParams::default();
        assert_eq!(p.progression, ProgressionOrder::Rpcl);
        assert_eq!(p.code_block, (64, 64));
        assert!(p.emit_tlm_plt, "indexing markers on by default");
    }
}
