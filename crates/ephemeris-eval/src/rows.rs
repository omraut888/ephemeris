//! One scored forecast per row, the unit every table is aggregated from.
//!
//! Values are in the account's *normalized* units (paper/PLAN.md §8
//! decision 2); `k` is the factor that normalized them, so a value in the
//! account's own currency is `value / k`. Coverage and PIT do not depend on
//! the units.

use std::fmt::Write as _;

use crate::dist::Score;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub account: u64,
    /// ISO date of the forecast origin.
    pub origin: String,
    pub h: i32,
    pub method: String,
    pub k: f64,
    pub truth: f64,
    pub score: Score,
    /// Strata, as of the origin.
    pub income_detected: bool,
    pub months_history: i32,
    /// A regime change was detected in the 28 days before the origin.
    pub alarm_recent: bool,
}

pub const HEADER: &str = "account,origin,h,method,k,truth,crps,in50,in80,in90,in95,width90,median,abs_err,pit,income,months,alarm";

impl Row {
    pub fn csv(&self) -> String {
        let s = &self.score;
        let b = |x: bool| u8::from(x);
        let mut out = String::new();
        write!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            self.account,
            self.origin,
            self.h,
            self.method,
            self.k,
            self.truth,
            s.crps,
            b(s.inside[0]),
            b(s.inside[1]),
            b(s.inside[2]),
            b(s.inside[3]),
            s.width90,
            s.median,
            s.abs_err,
            s.pit,
            b(self.income_detected),
            self.months_history,
            b(self.alarm_recent),
        )
        .expect("string write");
        out
    }

    pub fn parse(line: &str) -> Result<Self, String> {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != 18 {
            return Err(format!("{} fields: {line}", f.len()));
        }
        let num = |i: usize| f[i].parse::<f64>().map_err(|e| format!("{}: {e}", f[i]));
        let flag = |i: usize| match f[i] {
            "0" => Ok(false),
            "1" => Ok(true),
            x => Err(format!("flag {x}")),
        };
        Ok(Row {
            account: f[0].parse().map_err(|e| format!("{e}"))?,
            origin: f[1].to_string(),
            h: f[2].parse().map_err(|e| format!("{e}"))?,
            method: f[3].to_string(),
            k: num(4)?,
            truth: num(5)?,
            score: Score {
                crps: num(6)?,
                inside: [flag(7)?, flag(8)?, flag(9)?, flag(10)?],
                width90: num(11)?,
                median: num(12)?,
                abs_err: num(13)?,
                pit: num(14)?,
            },
            income_detected: flag(15)?,
            months_history: f[16].parse().map_err(|e| format!("{e}"))?,
            alarm_recent: flag(17)?,
        })
    }
}

pub fn read_rows(text: &str) -> Result<Vec<Row>, String> {
    let mut lines = text.lines();
    if lines.next() != Some(HEADER) {
        return Err("unexpected header".into());
    }
    lines.map(Row::parse).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let r = Row {
            account: 42,
            origin: "1996-05-01".into(),
            h: 30,
            method: "B-SA+".into(),
            k: 0.123_456_789,
            truth: -1.5e3,
            score: Score {
                crps: 12.25,
                inside: [false, true, true, true],
                width90: 1e-7,
                median: 3.0,
                abs_err: 0.1 + 0.2,
                pit: 0.5,
            },
            income_detected: true,
            months_history: 13,
            alarm_recent: false,
        };
        let text = format!("{HEADER}\n{}\n", r.csv());
        assert_eq!(read_rows(&text).unwrap(), vec![r]);
    }
}
