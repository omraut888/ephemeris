//! A compact ASCII rendering of the forecast cone, from a report's quantiles.
//!
//! Each column is one forecast day (sampled evenly when the horizon is wider
//! than the chart). `.` spans the outermost band, `:` the next one in, `*`
//! marks the median, and `-` the $0 line where nothing else is drawn.

use ephemeris_core::schema::ConeDay;

const MAX_WIDTH: usize = 60;
const HEIGHT: usize = 15;
const LABEL_WIDTH: usize = 9;

pub fn render(levels: &[f64], days: &[ConeDay]) -> String {
    if days.is_empty() || levels.is_empty() {
        return String::new();
    }
    let n = levels.len();
    let width = days.len().min(MAX_WIDTH);
    let columns: Vec<&ConeDay> = (0..width)
        .map(|j| &days[(j + 1) * days.len() / width - 1])
        .collect();

    let (mut bottom, mut top) = columns
        .iter()
        .flat_map(|d| [d.quantiles[0], d.quantiles[n - 1]])
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
            (lo.min(q), hi.max(q))
        });
    if top - bottom < 1.0 {
        (bottom, top) = (bottom - 1.0, top + 1.0);
    }
    let row = |v: f64| -> usize {
        let r = ((top - v) / (top - bottom) * (HEIGHT - 1) as f64).round();
        r.clamp(0.0, (HEIGHT - 1) as f64) as usize
    };

    let mut grid = vec![vec![' '; width]; HEIGHT];
    let zero_line = (bottom..=top).contains(&0.0);
    if zero_line {
        grid[row(0.0)].fill('-');
    }
    let median = levels.iter().position(|&l| l == 0.5);
    for (j, day) in columns.iter().enumerate() {
        let q = &day.quantiles;
        let mut band = |lo: f64, hi: f64, ch: char| {
            for r in grid.iter_mut().take(row(lo) + 1).skip(row(hi)) {
                r[j] = ch;
            }
        };
        if n >= 2 {
            band(q[0], q[n - 1], '.');
        }
        if n >= 4 {
            band(q[1], q[n - 2], ':');
        }
        if let Some(m) = median {
            grid[row(q[m])][j] = '*';
        }
    }

    let mut out = String::new();
    for (r, cells) in grid.iter().enumerate() {
        let label = match r {
            0 => short_money(top),
            r if r == HEIGHT / 2 => short_money((top + bottom) / 2.0),
            r if r == HEIGHT - 1 => short_money(bottom),
            _ => String::new(),
        };
        let line: String = cells.iter().collect();
        out.push_str(&format!("{label:>LABEL_WIDTH$} |{}\n", line.trim_end()));
    }
    out.push_str(&format!("{:>LABEL_WIDTH$} +{}\n", "", "-".repeat(width)));
    let first = columns[0].date.to_string();
    let last = columns[width - 1].date.to_string();
    let gap = (width + 1).saturating_sub(first.len() + last.len()).max(1);
    out.push_str(&format!(
        "{:>LABEL_WIDTH$}  {first}{}{last}\n",
        "",
        " ".repeat(gap)
    ));

    let pct = |l: f64| format!("{}%", 100.0 * l);
    let mut legend = Vec::new();
    if n >= 2 {
        legend.push(format!(". {}-{}", pct(levels[0]), pct(levels[n - 1])));
    }
    if n >= 4 {
        legend.push(format!(": {}-{}", pct(levels[1]), pct(levels[n - 2])));
    }
    if median.is_some() {
        legend.push("* median".to_string());
    }
    if zero_line {
        legend.push("- $0".to_string());
    }
    out.push_str(&format!("{:>LABEL_WIDTH$}  {}\n", "", legend.join("   ")));
    out
}

/// `$27.4k`, `-$950`, `$1.2M`.
fn short_money(x: f64) -> String {
    let sign = if x < 0.0 { "-" } else { "" };
    let a = x.abs();
    if a >= 1e6 {
        format!("{sign}${:.1}M", a / 1e6)
    } else if a >= 1e3 {
        format!("{sign}${:.1}k", a / 1e3)
    } else {
        format!("{sign}${a:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeris_core::schema::Date;

    fn days(values: impl Iterator<Item = [f64; 5]>) -> Vec<ConeDay> {
        values
            .enumerate()
            .map(|(i, q)| ConeDay {
                date: Date(19_723 + i as i32),
                quantiles: q.to_vec(),
            })
            .collect()
    }

    const LEVELS: [f64; 5] = [0.05, 0.25, 0.5, 0.75, 0.95];

    /// Median marks in the plot area (the legend also has one).
    fn stars(chart: &str) -> usize {
        chart
            .lines()
            .take(HEIGHT)
            .map(|l| l.matches('*').count())
            .sum()
    }

    #[test]
    fn widening_cone_shape() {
        // Median at 1000, bands widening by 10/day and 5/day.
        let d = days((0..30).map(|i| {
            let (o, n) = (10.0 * i as f64, 5.0 * i as f64);
            [1000.0 - o, 1000.0 - n, 1000.0, 1000.0 + n, 1000.0 + o]
        }));
        let chart = render(&LEVELS, &d);
        let lines: Vec<&str> = chart.lines().collect();
        assert_eq!(lines.len(), HEIGHT + 3);
        assert!(chart.is_ascii());
        // The median is a flat line through the middle row, one '*' per day.
        assert_eq!(lines[HEIGHT / 2].matches('*').count(), 30);
        assert_eq!(stars(&chart), 30);
        // The first day is a point; the last spans the full height.
        let col = |j: usize| -> String {
            lines[..HEIGHT]
                .iter()
                .map(|l| l.chars().nth(LABEL_WIDTH + 2 + j).unwrap_or(' '))
                .collect()
        };
        assert_eq!(col(0).trim(), "*");
        assert!(col(29).starts_with('.') && col(29).ends_with('.'));
        assert!(col(29).contains(':'));
        assert!(lines[0].trim_start().starts_with("$1.3k |"), "{}", lines[0]);
        assert!(
            lines[HEIGHT - 1].trim_start().starts_with("$710 |"),
            "{}",
            lines[HEIGHT - 1]
        );
        assert!(
            lines[HEIGHT + 1].contains("2024-01-01") && lines[HEIGHT + 1].contains("2024-01-30")
        );
        assert_eq!(lines[HEIGHT + 2].trim(), ". 5%-95%   : 25%-75%   * median");
    }

    #[test]
    fn long_horizons_are_sampled_and_zero_line_drawn() {
        // A straight fall from +900 to −900 over 180 days, no spread.
        let d = days((0..180).map(|i| [900.0 - 10.0 * i as f64; 5]));
        let chart = render(&LEVELS, &d);
        let lines: Vec<&str> = chart.lines().collect();
        assert_eq!(stars(&chart), MAX_WIDTH);
        // The axis spans the sampled days, ending on the last one.
        assert!(lines[HEIGHT + 1].trim_end().ends_with("2024-06-28"));
        assert!(
            lines[HEIGHT / 2].contains('-'),
            "zero line: {}",
            lines[HEIGHT / 2]
        );
    }

    #[test]
    fn flat_and_degenerate_inputs_do_not_panic() {
        let flat = days((0..5).map(|_| [100.0; 5]));
        assert_eq!(stars(&render(&LEVELS, &flat)), 5);
        assert_eq!(render(&LEVELS, &[]), "");
        let one = [ConeDay {
            date: Date(0),
            quantiles: vec![5.0],
        }];
        assert_eq!(stars(&render(&[0.5], &one)), 1);
    }

    #[test]
    fn short_money_formatting() {
        assert_eq!(short_money(27_412.0), "$27.4k");
        assert_eq!(short_money(-950.2), "-$950");
        assert_eq!(short_money(1_250_000.0), "$1.2M");
    }
}
