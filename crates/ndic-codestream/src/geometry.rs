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

/// Precinct grid of resolution `res`: `(npx, npy)`, or `(0, 0)` for an empty resolution.
/// `(ppx, ppy)` are the precinct exponents in resolution coordinates.
#[must_use]
pub fn precinct_grid(
    width: usize,
    height: usize,
    levels: u8,
    res: u8,
    ppx: u8,
    ppy: u8,
) -> (usize, usize) {
    let (rw, rh) = level_dims(width, height, levels - res);
    if rw == 0 || rh == 0 {
        (0, 0)
    } else {
        (rw.div_ceil(1 << ppx.min(31)), rh.div_ceil(1 << ppy.min(31)))
    }
}

#[allow(clippy::too_many_arguments, clippy::similar_names)]
/// The aligned code-block sub-grid a precinct covers within a band:
/// `(bx0, by0, nx, ny)` in the band's global block grid.
///
/// `pp_log` are the precinct exponents in *band* coordinates (already
/// reduced by one for resolutions above 0); block sizes divide precinct
/// sizes, so precinct edges align with block boundaries.
#[must_use]
pub fn precinct_block_range(
    bw: usize,
    bh: usize,
    ppx_log: u8,
    ppy_log: u8,
    px: usize,
    py: usize,
    cbw: usize,
    cbh: usize,
) -> (usize, usize, usize, usize) {
    if bw == 0 || bh == 0 {
        return (0, 0, 0, 0);
    }
    let ppw = 1usize << ppx_log.min(31);
    let pph = 1usize << ppy_log.min(31);
    let x0 = px * ppw;
    let y0 = py * pph;
    if x0 >= bw || y0 >= bh {
        return (0, 0, 0, 0);
    }
    let x1 = (x0 + ppw).min(bw);
    let y1 = (y0 + pph).min(bh);
    let bx0 = x0 / cbw;
    let by0 = y0 / cbh;
    (bx0, by0, x1.div_ceil(cbw) - bx0, y1.div_ceil(cbh) - by0)
}

/// The packet visit order `(comp, res, px, py)` of a single-tile, single-layer codestream
/// anchored at the origin (T.800 §B.12). `progression` is the `SGcod` code, `grid(r)` gives
/// resolution `r`'s precinct grid and `exp(r)` its precinct exponents.
///
/// The position-major orders visit a packet where its precinct's origin falls on the
/// reference grid, `(px, py)` scaled by `2^(PP + levels - r)`, so they sort by that origin.
pub fn packet_sequence(
    progression: u8,
    ncomps: usize,
    levels: u8,
    grid: impl Fn(u8) -> (usize, usize),
    exp: impl Fn(u8) -> (u8, u8),
) -> Vec<(usize, u8, usize, usize)> {
    let mut order = Vec::new();
    // Resolution-major raster of each resolution's precincts, with component inside or outside
    // the position loop.
    for r in 0..=levels {
        let (npx, npy) = grid(r);
        if progression == 2 {
            // RPCL: resolution, position, component.
            for py in 0..npy {
                for px in 0..npx {
                    for c in 0..ncomps {
                        order.push((c, r, px, py));
                    }
                }
            }
        } else {
            // LRCP/RLCP with one layer: resolution, component, position. PCRL/CPRL are sorted
            // below.
            for c in 0..ncomps {
                for py in 0..npy {
                    for px in 0..npx {
                        order.push((c, r, px, py));
                    }
                }
            }
        }
    }
    let origin = |r: u8, px: usize, py: usize| {
        let (ppx, ppy) = exp(r);
        let scale = |pp: u8| u32::from(pp) + u32::from(levels - r);
        (
            (py as u64) << scale(ppy).min(63),
            (px as u64) << scale(ppx).min(63),
        )
    };
    match progression {
        // PCRL: position, component, resolution.
        3 => order.sort_by_key(|&(c, r, px, py)| (origin(r, px, py), c, r)),
        // CPRL: component, position, resolution.
        4 => order.sort_by_key(|&(c, r, px, py)| (c, origin(r, px, py), r)),
        _ => {}
    }
    order
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
    fn position_major_orders_follow_the_reference_grid() {
        // Two levels with maximal precincts, where resolution 2 splits into three precincts and
        // resolution 1 into two. On the reference grid resolution 2's precincts start at 0,
        // 2^15 and 2^16, resolution 1's at 0 and 2^16, and resolution 0's at 0.
        let grid = |r: u8| [(1, 1), (2, 1), (3, 1)][usize::from(r)];
        let exp = |_| (15, 15);
        assert_eq!(
            packet_sequence(3, 1, 2, grid, exp),
            [
                (0, 0, 0, 0),
                (0, 1, 0, 0),
                (0, 2, 0, 0),
                (0, 2, 1, 0),
                (0, 1, 1, 0),
                (0, 2, 2, 0)
            ]
        );
        // PCRL takes every component at a position before the next position.
        let pcrl = packet_sequence(3, 2, 2, grid, exp);
        assert_eq!(
            pcrl[..4],
            [(0, 0, 0, 0), (0, 1, 0, 0), (0, 2, 0, 0), (1, 0, 0, 0)]
        );
        // CPRL finishes a component's positions before the next component.
        let cprl = packet_sequence(4, 2, 2, grid, exp);
        assert_eq!(cprl[..6], packet_sequence(3, 1, 2, grid, exp)[..]);
        assert!(cprl[6..].iter().all(|p| p.0 == 1));
        // RPCL runs each resolution's precincts in raster order.
        assert_eq!(
            packet_sequence(2, 1, 2, grid, exp),
            [
                (0, 0, 0, 0),
                (0, 1, 0, 0),
                (0, 1, 1, 0),
                (0, 2, 0, 0),
                (0, 2, 1, 0),
                (0, 2, 2, 0)
            ]
        );
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
