//! Transforms: where the four corners of a source rectangle land (a quad,
//! a projective map: move, scale, rotate, skew, distort, perspective),
//! optionally bent by a smooth grid first (warp). Pixels are resampled
//! premultiplied (nearest, bilinear or Photoshop's bicubic) from a
//! pyramid built when a reduction needs it, so big reductions don't
//! alias, on all cores.
//!
//! `homography`: the projective map and the quad's rules; `mesh`: the
//! grid and the whole warp; `source`: what is transformed, its pyramid
//! and sampling; `render`: drawing it over a destination.

// Pixel code indexes channels side by side.
#![allow(clippy::needless_range_loop)]

mod homography;
mod mesh;
mod render;
mod source;

pub use homography::{convex, corners, keep_convex, Homography, Quad};
pub use mesh::{Mesh, Warp, WarpMap};
pub use render::render;
pub use source::{Interp, Source};

#[cfg(test)]
mod tests;
