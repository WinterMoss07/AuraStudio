//! The animation tools: what you do to sheets, cels and animation folders,
//! and the playback cache. The model they work on (the tree, the time, the
//! onion skin ghosts the merge draws) stays in `aurastudio-layers`.
//!
//! The tools are traits on `LayerStack`: `use aurastudio_animation::prelude::*;`.

pub mod play;

pub mod prelude {
    pub use crate::play::PlayBuild;
}
