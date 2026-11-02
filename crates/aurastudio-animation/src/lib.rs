//! What time DOES to the tree: the frame shown, which sheet of each
//! animation folder shows then (`Timeline::settle`, called by the
//! document after every edit), sheet and cel operations (`SheetEdit`),
//! the onion skin's ghosts, where a stroke goes in an animation folder,
//! and the 2D camera. `layers` holds the animation folders as data and
//! never learns that time exists.
//!
//! Frames are the time (the timeline's columns, the X-sheet's rows);
//! sheets are the drawings exposed on them.

pub mod camera;
mod onion;
mod sheets;
mod timeline;

pub use camera::{Camera, CameraKey};
pub use onion::{Onion, OnionLook};
pub use sheets::{SheetEdit, Sheets};
pub use timeline::{Timeline, EVERYWHERE};

#[cfg(test)]
mod tests;
