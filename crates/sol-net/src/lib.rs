//! `sol-net` — peer-to-peer networking for Sea of Lost Souls.
//!
//! STUB. This crate will host the deterministic **lockstep** netcode (the
//! Age-of-Empires model) over WebRTC data channels via `matchbox`:
//!
//! - Command collection per simulation turn (orders, not world state).
//! - Turn scheduling with input delay (commands scheduled N turns ahead so
//!   every peer has all commands before simulating a turn).
//! - Desync detection via periodic `sol-sim` state checksums, with a one-time
//!   state-resync fallback from the lowest-peer-id authority.
//!
//! It depends on `sol-sim` being bit-deterministic across native and wasm.
//! See `design.md` §13. Intentionally near-empty for now.

#![forbid(unsafe_code)]
