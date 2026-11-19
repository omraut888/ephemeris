// The 100 generated cases of the extended byte-identity check
// (paper/PLAN.md §1 claim C1, table T10), shared by `identity.rs` (native,
// and inside WASM) and the FFI crate's test. A case is a simulated feed
// plus an option set, and every input is a fixed function of the case
// number. The cases vary:
// - the generator (realistic or model-matched);
// - the history length (40 to 1,000 days) and its start date (2019–2024);
// - the feed's holiday calendar (`us` by default, `cz`, `every_day`);
// - the horizon (30, 90, 180 or 365 days), the number of paths (200 to
//   2,000), the seed, and the targets.
// `identity-manifest.tsv` holds the SHA-256 of each case's feed and report
// bytes as computed natively on the Mac.

const IDENTITY_CASES: u32 = 100;

/// `(simulate options, forecast options, holidays)` of case `i`.
fn identity_inputs(i: u32) -> (String, String, Option<&'static str>) {
    let mode = if i.is_multiple_of(2) { "realistic" } else { "matched" };
    let days = 40 + (i * 37) % 961;
    // 2019-01-01 plus a spread of up to about four years.
    let start = ephemeris_core::schema::Date(17_897 + ((i * 53) % 1_500) as i32);
    let simulate = format!(r#"{{"seed": {}, "days": {days}, "mode": "{mode}", "start": "{start}"}}"#, 1_000 + i);
    let horizon = [30, 90, 180, 365][(i % 4) as usize];
    let paths = [200, 500, 1000, 2000][((i / 4) % 4) as usize];
    let targets = match i % 3 {
        0 => "[]",
        1 => "[0]",
        _ => "[1000, 5000]",
    };
    let forecast = format!(
        r#"{{"horizon_days": {horizon}, "paths": {paths}, "seed": {}, "targets": {targets}}}"#,
        u64::from(i) * 7_919 + 1
    );
    let holidays = match i % 5 {
        3 => Some("cz"),
        4 => Some("every_day"),
        _ => None,
    };
    (simulate, forecast, holidays)
}

/// The feed text the forecast reads: the simulated feed, with the case's
/// holiday calendar set when it has one.
fn with_holidays(feed: &str, holidays: Option<&str>) -> String {
    match holidays {
        None => feed.to_string(),
        Some(h) => {
            let rest = feed.strip_prefix('{').expect("a feed is a JSON object");
            format!(r#"{{"holidays":"{h}",{rest}"#)
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// One manifest line: `case<TAB>feed sha256<TAB>report sha256`.
fn identity_line(
    i: u32,
    simulate: impl Fn(&str) -> String,
    forecast: impl Fn(&str, &str) -> String,
) -> String {
    let (sim, opts, holidays) = identity_inputs(i);
    let feed = with_holidays(&simulate(&sim), holidays);
    let report = forecast(&feed, &opts);
    format!("{i}\t{}\t{}", sha256_hex(feed.as_bytes()), sha256_hex(report.as_bytes()))
}
