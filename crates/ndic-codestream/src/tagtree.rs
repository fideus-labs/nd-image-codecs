//! Tag trees for packet-header inclusion and missing-MSB coding
//! (T.800 §B.10.2), following the `OpenJPH` precinct formulation.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// A quad tag tree over a `w x h` grid of code-blocks, with per-node
/// values and sent/received flags. Level 0 holds the leaves; each higher
/// level halves the grid (ceil). One extra root level always reads 0.
#[derive(Debug)]
pub struct TagTree {
    /// Node values per level (level 0 = leaves).
    vals: Vec<Vec<u8>>,
    /// Sent/received flags per level.
    flags: Vec<Vec<u8>>,
    w: usize,
}

/// `ceil(log2(x))` for `x >= 1`.
#[must_use]
pub fn log2_ceil(x: usize) -> u32 {
    debug_assert!(x >= 1);
    let t = usize::BITS - 1 - x.leading_zeros();
    t + u32::from(!x.is_power_of_two())
}

impl TagTree {
    /// Number of levels above the leaves for a `w x h` grid.
    #[must_use]
    pub fn num_levels(w: usize, h: usize) -> u32 {
        1 + log2_ceil(w).max(log2_ceil(h))
    }

    /// Creates a tree with all leaf values `init` (255 = "unset" for
    /// encoder-side min reduction; 0 for the parse side).
    #[must_use]
    pub fn new(w: usize, h: usize, init: u8) -> Self {
        let num_levels = Self::num_levels(w, h) as usize;
        let mut vals = Vec::with_capacity(num_levels + 1);
        let mut flags = Vec::with_capacity(num_levels + 1);
        for lev in 0..=num_levels {
            let lw = w.div_ceil(1 << lev);
            let lh = h.div_ceil(1 << lev);
            vals.push(vec![if lev == 0 { init } else { 0 }; lw.max(1) * lh.max(1)]);
            flags.push(vec![0u8; lw.max(1) * lh.max(1)]);
        }
        Self { vals, flags, w }
    }

    /// Number of levels above the leaves.
    #[must_use]
    pub fn levels(&self) -> usize {
        self.vals.len() - 1
    }

    fn idx(&self, x: usize, y: usize, lev: usize) -> usize {
        let lw = self.w.div_ceil(1 << lev).max(1);
        y * lw + x
    }

    /// Reads the node value at (`x`, `y`) of `lev` (leaf coordinates are
    /// shifted right by `lev`).
    #[must_use]
    pub fn val(&self, x: usize, y: usize, lev: usize) -> u8 {
        let i = self.idx(x, y, lev);
        self.vals[lev][i]
    }

    /// Writes the node value.
    pub fn set_val(&mut self, x: usize, y: usize, lev: usize, v: u8) {
        let i = self.idx(x, y, lev);
        self.vals[lev][i] = v;
    }

    /// Reads the sent/received flag.
    #[must_use]
    pub fn flag(&self, x: usize, y: usize, lev: usize) -> u8 {
        let i = self.idx(x, y, lev);
        self.flags[lev][i]
    }

    /// Sets the sent/received flag.
    pub fn set_flag(&mut self, x: usize, y: usize, lev: usize) {
        let i = self.idx(x, y, lev);
        self.flags[lev][i] = 1;
    }

    /// Encoder-side: min-reduces leaf values up the tree (leaves must be
    /// filled first; the root level stays 0).
    pub fn reduce(&mut self) {
        for lev in 1..=self.levels() {
            let lw = self.w.div_ceil(1 << lev).max(1);
            let n = self.vals[lev].len();
            for i in 0..n {
                let x = i % lw;
                let y = i / lw;
                let mut m = u8::MAX;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let cx = 2 * x + dx;
                    let cy = 2 * y + dy;
                    let clw = self.w.div_ceil(1 << (lev - 1)).max(1);
                    let ch = self.vals[lev - 1].len().div_ceil(clw);
                    if cx < clw && cy < ch {
                        m = m.min(self.vals[lev - 1][cy * clw + cx]);
                    }
                }
                if lev == self.levels() {
                    // The synthetic root above the top level always reads 0.
                    self.vals[lev][i] = 0;
                } else {
                    self.vals[lev][i] = m;
                }
            }
        }
        // Root value is 0 by construction (single entry at the top).
        let top = self.levels();
        self.vals[top].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log2_ceil_values() {
        assert_eq!(log2_ceil(1), 0);
        assert_eq!(log2_ceil(2), 1);
        assert_eq!(log2_ceil(3), 2);
        assert_eq!(log2_ceil(8), 3);
        assert_eq!(log2_ceil(9), 4);
    }

    #[test]
    fn reduce_min_propagates() {
        let mut t = TagTree::new(3, 2, 255);
        for (i, v) in [5u8, 3, 7, 2, 9, 4].iter().enumerate() {
            t.set_val(i % 3, i / 3, 0, *v);
        }
        t.reduce();
        // Level 1 grid is 2x1: min(5,3,2,9)=2, min(7,4)=4.
        assert_eq!(t.val(0, 0, 1), 2);
        assert_eq!(t.val(1, 0, 1), 4);
        // Top level always 0.
        let top = t.levels();
        assert_eq!(t.val(0, 0, top), 0);
    }
}
