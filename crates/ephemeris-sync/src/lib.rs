//! EPHEMERIS sync: the state every device shares, and the feed built from it
//! (docs/phase4b-design.md).
//!
//! Pure, like the engine: no I/O, no clock, no randomness. Shells pass in
//! bytes, the time and nonces, and get bytes and JSON back.

#![forbid(unsafe_code)]

pub mod bank;
pub mod edits;
pub mod envelope;
pub mod hlc;
pub mod ingest;
pub mod json;
pub mod lww;
pub mod state;
pub mod tag;

pub use bank::{AccountSnapshot, BankRecord, Category};
pub use hlc::{Hlc, ReplicaId};
pub use state::SyncState;
