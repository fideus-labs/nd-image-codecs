//! Resolution, subband and code-block geometry for origin-anchored planes
//! (T.800 §B.5-B.7 specialized to `XOsiz = YOsiz = 0`).

extern crate alloc;

use alloc::vec::Vec;

use ndic_htj2k::dwt::level_dims;

/// One subband of one resolution, with its position in the Mallat-layout
/// coefficient plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    /// Resolution number (0 = LL only).
    pub res: u8,
    /// Band code: 0 = LL, 1 = HL, 2 = LH, 3 = HH.
    pub band: u8,
    /// X of the band's top-left corner in the Mallat plane.
    pub x0: usize,
    /// Y of the band's top-left corner in the Mallat plane.
    pub y0: usize,
    /// Band width (may be 0 for degenerate planes).
    pub w: usize,
    /// Band height.
    pub h: usize,
}

impl Band {
    /// Code-block grid: `(nx, ny)` for `cb = (cbw, cbh)`.
    #[must_use]
    pub fn grid(&self, cbw: usize, cbh: usize) -> (usize, usize) {
        if self.w == 0 || self.h == 0 {
            (0, 0)
        } else {
            (self.w.div_ceil(cbw), self.h.div_ceil(cbh))
        }
    }

    /// The rectangle of block (`bx`, `by`): `(x, y, w, h)` in band-local
    /// coordinates.
    #[must_use]
    pub fn block_rect(&self, bx: usize, by: usize, cbw: usize, cbh: usize) -> BlockRect {
        let x = bx * cbw;
        let y = by * cbh;
        BlockRect {
            x,
            y,
            w: cbw.min(self.w - x),
            h: cbh.min(self.h - y),
        }
    }
}

/// A code-block rectangle in band-local coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRect {
    /// Left edge.
    pub x: usize,
    /// Top edge.
    pub y: usize,
    /// Width.
    pub w: usize,
    /// Height.
    pub h: usize,
}

/// The subbands of resolution `res` for a `width x height` plane with
/// `levels` decompositions, in packet order (LL, or HL, LH, HH).
#[must_use]
pub fn bands_of_resolution(width: usize, height: usize, levels: u8, res: u8) -> Vec<Band> {
    let mut out = Vec::with_capacity(3);
    if res == 0 {
        let (w, h) = level_dims(width, height, levels);
        out.push(Band {
            res: 0,
            band: 0,
            x0: 0,
            y0: 0,
            w,
            h,
        });
        return out;
    }
    // Resolution r > 0 holds the detail bands of decomposition level
    // l = levels - r + 1, split from the level (l-1) LL region.
    let l = levels - res + 1;
    let (wp, hp) = level_dims(width, height, l - 1);
    let (wl, hl) = level_dims(width, height, l);
    let (wh, hh) = (wp - wl, hp - hl); // high-pass extents
    out.push(Band {
        res,
        band: 1,
        x0: wl,
        y0: 0,
        w: wh,
        h: hl,
    });
    out.push(Band {
        res,
        band: 2,
        x0: 0,
        y0: hl,
        w: wl,
        h: hh,
    });
    out.push(Band {
        res,
        band: 3,
        x0: wl,
        y0: hl,
        w: wh,
        h: hh,
    });
    out
}

/// Effective code-block size within a resolution: the nominal size is
/// halved at resolutions above 0 no further than the precinct allows; with
/// maximal precincts only the nominal size applies (T.800 §B.7).
#[must_use]
pub fn effective_cb(cbw: usize, cbh: usize, ppx: u8, ppy: u8, res: u8) -> (usize, usize) {
    // Precinct size in band coordinates is halved for res > 0.
    let shift = usize::from(res > 0);
    let pw = 1usize << (usize::from(ppx) - shift).min(31);
    let ph = 1usize << (usize::from(ppy) - shift).min(31);
    (cbw.min(pw), cbh.min(ph))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_tile_the_plane() {
        // 65 x 33 with 2 levels: LL 17x9 | r1: HL 16x9, LH 17x8, HH 16x8 |
        // r2: HL 32x17, LH 33x16, HH 32x16.
        let ll = bands_of_resolution(65, 33, 2, 0);
        assert_eq!((ll[0].w, ll[0].h), (17, 9));
        let r1 = bands_of_resolution(65, 33, 2, 1);
        assert_eq!((r1[0].w, r1[0].h), (16, 9));
        assert_eq!((r1[0].x0, r1[0].y0), (17, 0));
        assert_eq!((r1[1].w, r1[1].h), (17, 8));
        assert_eq!((r1[2].w, r1[2].h), (16, 8));
        let r2 = bands_of_resolution(65, 33, 2, 2);
        assert_eq!((r2[0].w, r2[0].h), (32, 17));
        assert_eq!((r2[1].w, r2[1].h), (33, 16));
        assert_eq!((r2[2].w, r2[2].h), (32, 16));
    }

    #[test]
    fn degenerate_bands_have_zero_blocks() {
        // A 1-wide plane has empty HL/HH bands.
        let r1 = bands_of_resolution(1, 8, 1, 1);
        assert_eq!(r1[0].grid(64, 64), (0, 0));
        assert_eq!(r1[1].grid(64, 64), (1, 1));
    }

    #[test]
    fn block_rects_clip() {
        let b = Band {
            res: 1,
            band: 1,
            x0: 0,
            y0: 0,
            w: 100,
            h: 70,
        };
        assert_eq!(b.grid(64, 64), (2, 2));
        let r = b.block_rect(1, 1, 64, 64);
        assert_eq!((r.w, r.h), (36, 6));
    }
}
