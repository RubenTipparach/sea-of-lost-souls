//! `sol-procgen` — seeded procedural generation for Sea of Lost Souls.
//!
//! STUB. This crate will host the seeded generators shared by `sol-sim`
//! (deterministic gameplay geometry/fields: gravity wells, sensor occlusion,
//! hazard intensity) and `sol-render` (visual elaboration: high-res
//! volumetrics, asteroid meshes, planet shaders).
//!
//! The deterministic core (consumed by the simulation) must be reproducible
//! from a seed across all peers; the visual layer derived from the same seed
//! may be device-dependent because it never feeds back into gameplay.
//! See `design.md` §7. Intentionally near-empty for now.

#![forbid(unsafe_code)]
