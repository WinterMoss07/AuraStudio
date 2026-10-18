//! Selections: which pixels painting, filling and clearing may change.
//!
//! Stored as one bit per pixel (a B4 page at 600 dpi: 6.5 MB), plus the
//! outline it was made from, to draw it.

use aurastudio_canvas::Rect;

#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    width:    u32,
    height:   u32,
    bits:     Vec<u64>,
    bbox:     Rect,
    /// Closed outlines, canvas pixels (drawn as marching ants).
    outlines: Vec<Vec<(f32, f32)>>,
    /// Exactly its bounding box (rectangle tool, select all).
    is_rect:  bool,
}

impl Selection {
    fn empty(width: u32, height: u32) -> Self {
        let n = (width as usize * height as usize).div_ceil(64);
        Self { width, height, bits: vec![0; n], bbox: (0, 0, 0, 0), outlines: Vec::new(), is_rect: false }
    }

    /// The whole canvas.
    pub fn all(width: u32, height: u32) -> Self {
        Self::rect(width, height, (0, 0, width, height)).expect("canvas not empty")
    }

    /// A rectangle (clipped to the canvas); None if nothing is left.
    pub fn rect(width: u32, height: u32, r: Rect) -> Option<Self> {
        let (x0, y0, x1, y1) = (r.0.min(width), r.1.min(height), r.2.min(width), r.3.min(height));
        if x0 >= x1 || y0 >= y1 { return None; }
        let mut s = Self::empty(width, height);
        for y in y0..y1 { s.set_run(y, x0, x1); }
        s.bbox = (x0, y0, x1, y1);
        let (fx0, fy0, fx1, fy1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
        s.outlines = vec![vec![(fx0, fy0), (fx1, fy0), (fx1, fy1), (fx0, fy1)]];
        s.is_rect = true;
        Some(s)
    }

    /// Inside a closed freehand outline (lasso), even-odd rule, a pixel
    /// counting when its center is inside. None if it covers no pixel.
    pub fn polygon(width: u32, height: u32, points: &[(f32, f32)]) -> Option<Self> {
        if points.len() < 3 { return None; }
        let finite = points.iter().all(|p| p.0.is_finite() && p.1.is_finite());
        if !finite { return None; }
        let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
        for p in points { ymin = ymin.min(p.1); ymax = ymax.max(p.1); }
        let y0 = (ymin.floor().max(0.0) as u32).min(height);
        let y1 = ((ymax.ceil().max(0.0)) as u32).min(height);
        let mut s = Self::empty(width, height);
        let (mut bx0, mut bx1, mut by0, mut by1) = (u32::MAX, 0, u32::MAX, 0);
        let mut xs = Vec::new();
        for y in y0..y1 {
            let cy = y as f32 + 0.5;
            xs.clear();
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                if (a.1 <= cy) != (b.1 <= cy) {
                    xs.push(a.0 + (cy - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
            xs.sort_by(f32::total_cmp);
            for pair in xs.chunks_exact(2) {
                // Pixels whose center is between the two crossings.
                let x0 = ((pair[0] - 0.5).ceil().max(0.0) as u32).min(width);
                let x1 = (((pair[1] - 0.5).floor() + 1.0).max(0.0) as u32).min(width);
                if x0 < x1 {
                    s.set_run(y, x0, x1);
                    (bx0, bx1, by0, by1) = (bx0.min(x0), bx1.max(x1), by0.min(y), by1.max(y + 1));
                }
            }
        }
        if bx0 >= bx1 { return None; }
        s.bbox = (bx0, by0, bx1, by1);
        s.outlines = vec![points.to_vec()];
        Some(s)
    }

    fn set_run(&mut self, y: u32, x0: u32, x1: u32) {
        let row = y as usize * self.width as usize;
        for i in row + x0 as usize..row + x1 as usize {
            self.bits[i / 64] |= 1 << (i % 64);
        }
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height { return false; }
        let i = y as usize * self.width as usize + x as usize;
        self.bits[i / 64] >> (i % 64) & 1 == 1
    }

    /// Smallest rectangle holding every selected pixel.
    pub fn bbox(&self) -> Rect { self.bbox }

    pub fn outlines(&self) -> &[Vec<(f32, f32)>] { &self.outlines }

    pub fn is_rect(&self) -> bool { self.is_rect }

    pub fn size(&self) -> (u32, u32) { (self.width, self.height) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_and_lasso_select_the_pixels_inside() {
        let r = Selection::rect(100, 100, (10, 20, 30, 40)).unwrap();
        assert!(r.contains(10, 20) && r.contains(29, 39) && !r.contains(30, 20) && !r.contains(9, 25));
        assert_eq!(r.bbox(), (10, 20, 30, 40));
        assert!(Selection::rect(100, 100, (200, 0, 300, 10)).is_none());

        // A triangle: its tip is narrow, its base wide.
        let t = Selection::polygon(100, 100, &[(50.0, 10.0), (90.0, 90.0), (10.0, 90.0)]).unwrap();
        assert!(t.contains(50, 50) && t.contains(15, 89));
        assert!(!t.contains(15, 15) && !t.contains(50, 95));
        // Outside the canvas: clipped, never out of bounds.
        let big = Selection::polygon(100, 100, &[(-50.0, -50.0), (500.0, -50.0), (500.0, 500.0)]).unwrap();
        assert!(big.contains(99, 99) && big.bbox().2 <= 100);
        assert!(Selection::polygon(100, 100, &[(1.0, 1.0), (2.0, 2.0)]).is_none());
        assert!(Selection::polygon(100, 100, &[(f32::NAN, 1.0), (2.0, 2.0), (3.0, 1.0)]).is_none());
    }
}
