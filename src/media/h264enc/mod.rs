//! Built-in H.264 encoder (Constrained Baseline, CAVLC).
//!
//! Sources: ITU-T H.264 (08/2021), see the submodules.

pub mod bits;
pub mod cavlc;
pub mod headers;
pub mod intra;
pub mod picture;
pub mod tables;
pub mod transform;
