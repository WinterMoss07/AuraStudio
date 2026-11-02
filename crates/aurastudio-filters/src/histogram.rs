//! Counts of each level among a layer's painted pixels (Levels' and
//! Threshold's dialogs).

use aurastudio_core::{Rect, Rgba8};
use aurastudio_raster::{Image, Mask, Run};

use crate::tone;

#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    /// R, G and B together (Levels' RGB channel).
    pub rgb:   [u32; 256],
    pub chan:  [[u32; 256]; 3],
    /// 30 R + 59 G + 11 B (what Threshold splits on).
    pub lum:   [u32; 256],
    pub count: u64,
}

impl Default for Histogram {
    fn default() -> Self { Histogram { rgb: [0; 256], chan: [[0; 256]; 3], lum: [0; 256], count: 0 } }
}

impl Histogram {
    /// `n` pixels of color `p` (transparent ones don't count).
    #[inline]
    fn add(&mut self, p: Rgba8, n: u32) {
        if p[3] == 0 || n == 0 { return; }
        for c in 0..3 { self.chan[c][p[c] as usize] += n; }
        self.lum[tone::luminosity([p[0], p[1], p[2]]) as usize] += n;
        self.count += n as u64;
    }

    fn finish(mut self) -> Self {
        for i in 0..256 { self.rgb[i] = self.chan[0][i] + self.chan[1][i] + self.chan[2][i]; }
        self
    }

    /// The histogram of the pixels `keep` accepts (by index) that aren't
    /// transparent.
    pub fn of(px: &[Rgba8], keep: impl Fn(usize) -> bool) -> Self {
        let mut h = Histogram::default();
        for (i, p) in px.iter().enumerate() { if keep(i) { h.add(*p, 1); } }
        h.finish()
    }

    /// The histogram of a layer's pixels in `rect`, inside `sel` (where it
    /// selects anything): runs of one color counted at once.
    pub fn of_layer(img: &Image, rect: Rect, sel: Option<&Mask>) -> Self {
        let mut h = Histogram::default();
        let mut mask = Vec::new();
        for y in rect.y0..rect.y1 {
            if let Some(s) = sel {
                mask.clear();
                s.row(y, rect.x0, rect.x1, |_, run| for i in 0..run.len() { mask.push(run.at(i)); });
            }
            img.row(y, rect.x0, rect.x1, |x, run| {
                let o = (x - rect.x0) as usize;
                match (&run, sel) {
                    (Run::Uniform(c, n), None) => h.add(*c, *n as u32),
                    // Line art: one color wherever something shows.
                    (Run::Coverage { color, alpha, .. }, None) => h.add(*color, alpha.iter().filter(|&&a| a > 0).count() as u32),
                    _ => for i in 0..run.len() {
                        if sel.is_none() || mask.get(o + i).is_some_and(|&m| m > 0) { h.add(run.at(i), 1); }
                    },
                }
            });
        }
        h.finish()
    }

    /// Levels' channel `ch` (0 RGB, 1..3 R, G, B).
    pub fn channel(&self, ch: usize) -> &[u32; 256] { if ch == 0 { &self.rgb } else { &self.chan[ch - 1] } }
}
