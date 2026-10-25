//! `ephemeris simulate`: a synthetic feed snapshot at the last generated day.

use std::path::Path;

use ephemeris_core::Day;
use ephemeris_core::TxnStatus;
use ephemeris_core::pipeline::{SimMode, SimulateOptions, simulate_feed};
use ephemeris_core::schema::Date;

use crate::Mode;
use crate::files::write_json;

pub fn run(seed: u64, days: u32, mode: Mode, start: Day, out: &Path) -> Result<(), String> {
    let opts = SimulateOptions {
        seed,
        days,
        mode: match mode {
            Mode::Realistic => SimMode::Realistic,
            Mode::Matched => SimMode::Matched,
        },
        start: Date(start),
    };
    let feed = simulate_feed(&opts).map_err(|e| format!("generator: {e}"))?;
    write_json(out, &feed)?;

    let pending = feed
        .transactions
        .iter()
        .filter(|t| t.status == TxnStatus::Pending)
        .count();
    println!(
        "Wrote {} transactions ({pending} pending), {} to {}, ledger balance {:.2}, to {}",
        feed.transactions.len(),
        feed.history_start,
        feed.today,
        feed.ledger_balance,
        out.display()
    );
    Ok(())
}
