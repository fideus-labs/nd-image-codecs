//! Tag trees for packet-header inclusion and missing-MSB coding
//! (T.800 §B.10.2), following the `OpenJPH` precinct formulation.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Room for every level a `usize` grid can have, plus the end offset.
const MAX_OFFSETS: usize = usize::BITS as usize + 3;

/// A quad tag tree over a `w x h` grid of code-blocks, with per-node
/// values and sent/received flags. Level 0 holds the leaves; each higher
/// level halves the grid (ceil). One extra root level always reads 0.
#[derive(Debug)]
pub struct TagTree {
    /// `[value, sent/received flag]` per node, every level back to back
    /// (level 0 = leaves), so a tree is one allocation however many levels
    /// it has.
    nodes: Vec<[u8; 2]>,
    /// Start of each level in [`Self::nodes`], then the end of the last.
    offsets: [usize; MAX_OFFSETS],
    /// Number of levels above the leaves.
    levels: usize,
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
        let levels = Self::num_levels(w, h) as usize;
        let mut offsets = [0; MAX_OFFSETS];
        let mut total = 0;
        for (lev, offset) in offsets.iter_mut().enumerate().take(levels + 1) {
            *offset = total;
            let lw = w.div_ceil(1 << lev);
            let lh = h.div_ceil(1 << lev);
            total += lw.max(1) * lh.max(1);
        }
        offsets[levels + 1] = total;
        let mut nodes = vec![[0u8; 2]; total];
        for node in &mut nodes[..offsets[1]] {
            node[0] = init;
        }
        Self {
            nodes,
            offsets,
            levels,
            w,
        }
    }

    /// Number of levels above the leaves.
    #[must_use]
    pub fn levels(&self) -> usize {
        self.levels
    }

    /// Number of nodes on `lev`.
    fn level_len(&self, lev: usize) -> usize {
        self.offsets[lev + 1] - self.offsets[lev]
    }

    fn idx(&self, x: usize, y: usize, lev: usize) -> usize {
        let lw = self.w.div_ceil(1 << lev).max(1);
        let i = y * lw + x;
        assert!(i < self.level_len(lev), "tag tree node outside its level");
        self.offsets[lev] + i
    }

    /// Reads the node value at (`x`, `y`) of `lev` (leaf coordinates are
    /// shifted right by `lev`).
    #[must_use]
    pub fn val(&self, x: usize, y: usize, lev: usize) -> u8 {
        let i = self.idx(x, y, lev);
        self.nodes[i][0]
    }

    /// Writes the node value.
    pub fn set_val(&mut self, x: usize, y: usize, lev: usize, v: u8) {
        let i = self.idx(x, y, lev);
        self.nodes[i][0] = v;
    }

    /// Reads the sent/received flag.
    #[must_use]
    pub fn flag(&self, x: usize, y: usize, lev: usize) -> u8 {
        let i = self.idx(x, y, lev);
        self.nodes[i][1]
    }

    /// Sets the sent/received flag.
    pub fn set_flag(&mut self, x: usize, y: usize, lev: usize) {
        let i = self.idx(x, y, lev);
        self.nodes[i][1] = 1;
    }

    /// Encoder-side: min-reduces leaf values up the tree (leaves must be
    /// filled first; the root level stays 0).
    pub fn reduce(&mut self) {
        for lev in 1..=self.levels() {
            let lw = self.w.div_ceil(1 << lev).max(1);
            let (below, here) = (self.offsets[lev - 1], self.offsets[lev]);
            let n = self.level_len(lev);
            for i in 0..n {
                let x = i % lw;
                let y = i / lw;
                let mut m = u8::MAX;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let cx = 2 * x + dx;
                    let cy = 2 * y + dy;
                    let clw = self.w.div_ceil(1 << (lev - 1)).max(1);
                    let ch = self.level_len(lev - 1).div_ceil(clw);
                    if cx < clw && cy < ch {
                        m = m.min(self.nodes[below + cy * clw + cx][0]);
                    }
                }
                if lev == self.levels() {
                    // The synthetic root above the top level always reads 0.
                    self.nodes[here + i][0] = 0;
                } else {
                    self.nodes[here + i][0] = m;
                }
            }
        }
        // Root value is 0 by construction (single entry at the top).
        let top = self.levels();
        for node in &mut self.nodes[self.offsets[top]..self.offsets[top + 1]] {
            node[0] = 0;
        }
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
