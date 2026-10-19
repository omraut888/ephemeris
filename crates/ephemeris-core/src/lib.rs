//! EPHEMERIS core engine: state estimation and forecasting over transaction data.
//!
//! Pure computation only: no I/O, no network, no printing (see CLAUDE.md).
//! Design: docs/phase1-design.md.

#![forbid(unsafe_code)]

pub mod adaptive;
pub mod calendar;
pub mod error;
pub mod kalman;
pub mod linalg;
pub mod model;
pub mod observe;
pub mod synth;
pub mod tracker;
pub mod types;

pub use error::EngineError;
pub use types::{
    AccountId, Channel, Day, DayInput, FlowKind, Observation, StreamId, Transaction, TxnId,
    TxnStatus,
};
