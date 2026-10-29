//! EPHEMERIS core engine: state estimation and forecasting over transaction data.
//!
//! Pure computation only: no I/O, no network, no printing (see CLAUDE.md).
//! Design: docs/phase1-design.md.

#![forbid(unsafe_code)]

pub mod adaptive;
pub mod calendar;
pub mod change;
pub mod error;
pub mod forecast;
pub mod kalman;
pub mod linalg;
pub mod metrics;
pub mod model;
pub mod observe;
#[cfg(feature = "serde")]
pub mod pipeline;
pub mod schedule;
#[cfg(feature = "serde")]
pub mod schema;
pub mod synth;
pub mod tracker;
pub mod truncated;
pub mod types;

pub use error::EngineError;
pub use types::{
    AccountId, Channel, Day, DayInput, FlowKind, Observation, StreamId, Transaction, TxnId,
    TxnStatus,
};
