//! The outline where coverage crosses one half, for the marching ants.

use aurastudio_core::{Point, Rect};

use crate::Selection;

/// A boundary piece of the outline, canvas pixels (horizontal or vertical).
pub type Segment = [Point; 2];

impl Selection {
    /// The outline as horizontal and vertical runs, looking at one pixel
    /// per `step` × `step` block (zoomed out, a fine outline is not seen
    /// anyway). Rows are read whole; cache it by `stamp()` and step.
    pub fn outline(&self, step: u32) -> Vec<Segment> {
        aurastudio_core::zone!("selection outline");
        let s = step.max(1) as i64;
        let b = self.bbox;
        if b.is_empty() { return Vec::new(); }
        let (bx0, by0, bx1, by1) = (b.x0 as i64, b.y0 as i64, b.x1 as i64, b.y1 as i64);
        // Cells over the box, one more all around (outside).
        let (gx0, gy0) = (bx0.div_euclid(s), by0.div_euclid(s));
        let (gx1, gy1) = ((bx1 + s - 1).div_euclid(s), (by1 + s - 1).div_euclid(s));
        let (w, h) = ((gx1 - gx0 + 2) as usize, (gy1 - gy0 + 2) as usize);
        let mut grid = vec![false; w * h];
        let xs: Vec<usize> = (1..w - 1).map(|gx| (((gx0 + gx as i64 - 1) * s + s / 2).clamp(bx0, bx1 - 1) - bx0) as usize).collect();
        for gy in 1..h - 1 {
            let y = ((gy0 + gy as i64 - 1) * s + s / 2).clamp(by0, by1 - 1) as i32;
            let row = self.mask.read(Rect::new(b.x0, y, b.x1, y + 1));
            for (k, &x) in xs.iter().enumerate() { grid[gy * w + k + 1] = row[x] >= 128; }
        }
        let at = |x: usize, y: usize| grid[y * w + x];
        // Cell edges → canvas pixels, kept inside the box.
        let px = |g: usize| ((gx0 + g as i64 - 1) * s).clamp(bx0, bx1) as f64;
        let py = |g: usize| ((gy0 + g as i64 - 1) * s).clamp(by0, by1) as f64;
        let mut out = Vec::new();
        for y in 1..h {
            let mut run = None;
            for x in 0..=w {
                let edge = x < w && at(x, y) != at(x, y - 1);
                match (edge, run) {
                    (true, None) => run = Some(x),
                    (false, Some(x0)) => { out.push([Point::new(px(x0), py(y)), Point::new(px(x), py(y))]); run = None; }
                    _ => {}
                }
            }
        }
        for x in 1..w {
            let mut run = None;
            for y in 0..=h {
                let edge = y < h && at(x, y) != at(x - 1, y);
                match (edge, run) {
                    (true, None) => run = Some(y),
                    (false, Some(y0)) => { out.push([Point::new(px(x), py(y0)), Point::new(px(x), py(y))]); run = None; }
                    _ => {}
                }
            }
        }
        out
    }
}
