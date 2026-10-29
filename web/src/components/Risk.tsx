// The headline risk: the chance the balance goes below $0 on any day of the
// horizon. The engine counts it (`below_by` for the target 0 the app always
// requests); this component only reads the last day and words it.

import { longDate, percent } from "../format";
import type { ForecastReport } from "../schema.gen";

/** The target the app adds to every forecast so the headline has a curve. */
export const OVERDRAWN_TARGET = 0;

/** Display tiers only; the probability itself comes from the engine. */
function tone(p: number): string {
  if (p >= 0.2) return "bad";
  if (p >= 0.05) return "warn";
  return "good";
}

export function Risk({ report }: { report: ForecastReport }) {
  const curve = report.forecast.targets.find((t) => t.amount === OVERDRAWN_TARGET);
  const p = curve?.below_by.at(-1);
  const last = report.forecast.days.at(-1);
  if (p === undefined || !last) return null;
  return (
    <div className={`risk ${tone(p)}`} data-testid="risk">
      <span className="big num">{percent(p)}</span>
      <span>
        chance the balance dips below $0 at any point through {longDate(last.date)}
        <span className="dim"> · end-of-day balances, {report.forecast.paths.toLocaleString("en-US")} simulated paths</span>
      </span>
    </div>
  );
}
