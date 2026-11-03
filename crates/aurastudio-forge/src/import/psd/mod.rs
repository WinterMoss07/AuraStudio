//! The parts of Photoshop's file structures brush files use: big-endian
//! reading, PackBits / ZIP planes, action descriptors, patterns and the
//! `.abr` layouts. From PhotoCraft's `crates/psd` (see NOTICE.md).

pub mod abr;
pub mod compression;
pub mod descriptor;
pub mod error;
pub mod header;
pub mod io;
pub mod patterns;
