//! Phase 6 evaluation (paper/PLAN.md): backtests on real and synthetic
//! data, the baselines, and the tables. Not part of any app; the engine
//! itself stays in `ephemeris-core`.

#![forbid(unsafe_code)]

pub mod aggregate;
pub mod baselines;
pub mod berka;
pub mod berka_run;
pub mod dist;
pub mod reference;
pub mod rows;
pub mod split;
