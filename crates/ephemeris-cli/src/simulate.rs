//! `ephemeris simulate`: a synthetic feed snapshot at the last generated day.

use std::path::Path;

use ephemeris_core::Day;
use ephemeris_core::TxnStatus;
use ephemeris_core::schema::{Date, Feed};
use ephemeris_core::synth::{
    INCOME_NAME, INCOME_STREAM, SynthConfig, generate, subscription_stream,
};

use crate::Mode;
use crate::files::write_json;

pub fn run(seed: u64, days: u32, mode: Mode, start: Day, out: &Path) -> Result<(), String> {
    let mut cfg = match mode {
        Mode::Realistic => SynthConfig::realistic(start),
        Mode::Matched => SynthConfig::model_matched(start),
    };
    cfg.n_days = days;
    let run = generate(&cfg, seed).map_err(|e| format!("generator: {e}"))?;
    let today = start + days as Day - 1;
    let txns = run.snapshot(today);
    let mut feed = Feed::new(start, today, run.ledger_balance(today), &txns);
    // Name the known streams as a bank feed's merchant names would.
    for t in &mut feed.transactions {
        t.merchant_name = match t.stream {
            Some(s) if s == INCOME_STREAM.0 => Some(INCOME_NAME.to_string()),
            Some(s) => cfg
                .subscriptions
                .iter()
                .enumerate()
                .find(|&(i, _)| subscription_stream(i).0 == s)
                .map(|(_, sub)| sub.name.to_string()),
            None => None,
        };
    }
    write_json(out, &feed)?;

    let pending = txns
        .iter()
        .filter(|t| t.status == TxnStatus::Pending)
        .count();
    println!(
        "Wrote {} transactions ({pending} pending), {} to {}, ledger balance {:.2}, to {}",
        txns.len(),
        Date(start),
        Date(today),
        feed.ledger_balance,
        out.display()
    );
    Ok(())
}
