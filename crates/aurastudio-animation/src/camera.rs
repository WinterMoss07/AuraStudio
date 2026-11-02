//! The 2D camera: keys on frames (where it looks, how close, how turned),
//! eased between them by a transfer curve. A data block the document holds
//! beside its timeline, not a tree node (the "Camera" column of the
//! X-sheet is drawn by the UI). Imports only `core`.

use aurastudio_core::{Affine, Point, TransferCurve};

/// The camera at one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraKey {
    pub frame:  u32,
    /// The canvas point at the middle of the shot.
    pub center: Point,
    /// Canvas pixels per output pixel: 1 at the document's size.
    pub zoom:   f64,
    /// Degrees, clockwise.
    pub angle:  f64,
    /// How the move toward the next key goes (0..1 time → 0..1 of the way).
    pub ease:   TransferCurve,
}

/// The keys of a shot, kept in frame order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Camera {
    keys: Vec<CameraKey>,
}

impl Camera {
    pub fn keys(&self) -> &[CameraKey] { &self.keys }

    /// Add or replace the key at its frame.
    pub fn set_key(&mut self, k: CameraKey) {
        match self.keys.binary_search_by_key(&k.frame, |x| x.frame) {
            Ok(i) => self.keys[i] = k,
            Err(i) => self.keys.insert(i, k),
        }
    }

    pub fn remove_key(&mut self, frame: u32) -> bool {
        match self.keys.binary_search_by_key(&frame, |x| x.frame) { Ok(i) => { self.keys.remove(i); true } Err(_) => false }
    }

    /// (center, zoom, angle) at frame `t`: held before the first key and
    /// after the last, eased between two. `None` without keys.
    pub fn at(&self, t: f64) -> Option<(Point, f64, f64)> {
        let i = self.keys.partition_point(|k| (k.frame as f64) <= t);
        let k = |k: &CameraKey| (k.center, k.zoom, k.angle);
        match (i.checked_sub(1).map(|j| &self.keys[j]), self.keys.get(i)) {
            (None, None) => None,
            (None, Some(b)) => Some(k(b)),
            (Some(a), None) => Some(k(a)),
            (Some(a), Some(b)) => {
                let s = ((t - a.frame as f64) / (b.frame - a.frame).max(1) as f64).clamp(0.0, 1.0);
                let e = a.ease.eval(s as f32) as f64;
                // Zoom eased in log space: a dolly looks even.
                let zoom = (a.zoom.max(1e-6).ln() * (1.0 - e) + b.zoom.max(1e-6).ln() * e).exp();
                Some((a.center.lerp(b.center, e), zoom, a.angle + (b.angle - a.angle) * e))
            }
        }
    }

    /// Canvas → output of `w` × `h` pixels at frame `t` (identity-like
    /// framing of the document without keys).
    pub fn transform(&self, t: f64, w: f64, h: f64) -> Affine {
        let (c, zoom, angle) = self.at(t).unwrap_or((Point::new(w / 2.0, h / 2.0), 1.0, 0.0));
        Affine::translate(-c.x, -c.y)
            .then(&Affine::rotate(-angle.to_radians()))
            .then(&Affine::scale(1.0 / zoom, 1.0 / zoom))
            .then(&Affine::translate(w / 2.0, h / 2.0))
    }
}
