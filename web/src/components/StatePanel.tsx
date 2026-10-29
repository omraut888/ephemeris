// The current state: the tracker's estimate with its ±2σ ranges, the
// detected pay schedule, the named bills, and any regime changes.

import { cadence, longDate, money, ordinal, shortDate, signedMoney } from "../format";
import type { BillReport, Estimate, ForecastReport } from "../schema.gen";

function Stat({
  label,
  e,
  unit = "",
}: {
  label: string;
  e: Estimate;
  unit?: string;
}) {
  return (
    <div className="stat">
      <span className="dim">{label}</span>
      <span className="value num">
        {money(e.mean)}
        <span className="dim">{unit}</span>
      </span>
      <span className="range num">
        ±2σ {money(e.low_2sd)} to {money(e.high_2sd)}
      </span>
    </div>
  );
}

function billCadence(b: BillReport): string {
  return b.monthly_on ? `monthly on the ${ordinal(b.monthly_on.day)}` : `every ${b.period_days} days`;
}

/** Detected streams of one direction; the engine signs amounts (inflow +). */
function RecurringList({ title, bills }: { title: "In" | "Out"; bills: BillReport[] }) {
  if (bills.length === 0) return null;
  const flow = title === "In" ? "inflow" : "outflow";
  return (
    <>
      <div className="sublabel">{title}</div>
      <ul className="list" aria-label={`Recurring ${flow}s`}>
        {bills.map((b) => (
          <li key={b.stream} className={flow}>
            <span>
              {b.name ?? `Stream ${b.stream}`}
              <span className="sub">
                {billCadence(b)} · next {shortDate(b.next_due)}
              </span>
            </span>
            <span className="num amount">{signedMoney(b.amount)}</span>
          </li>
        ))}
      </ul>
    </>
  );
}

export function StatePanel({ report }: { report: ForecastReport }) {
  const inc = report.income;
  return (
    <aside className="panel" aria-label="Current state">
      <section className="section">
        <div className="label">State · as of {longDate(report.as_of)}</div>
        <Stat label="Balance" e={report.balance} />
        <Stat label="Spend rate" e={report.spend_rate} unit="/day" />
        <Stat label="Paycheck" e={report.expected_paycheck} />
        <p className="dim" style={{ fontSize: 12, marginBottom: 0 }}>
          Settled through {longDate(report.settled_through)}; later days still hold pending amounts.
        </p>
      </section>

      <section className="section">
        <div className="label">Pay schedule</div>
        {inc ? (
          <ul className="list">
            <li>
              <span>
                {inc.name ?? "Paycheck"}
                <span className="sub">{cadence(inc.cadence)}</span>
              </span>
              <span className="num">
                {inc.next_scheduled ? `next ${shortDate(inc.next_scheduled)}` : "—"}
              </span>
            </li>
          </ul>
        ) : (
          <p className="dim">No regular income detected: the forecast has no paychecks.</p>
        )}
      </section>

      <section className="section">
        <div className="label">Recurring</div>
        {report.bills.length === 0 ? (
          <p className="dim">No recurring streams detected.</p>
        ) : (
          <>
            <RecurringList title="In" bills={report.bills.filter((b) => b.amount > 0)} />
            <RecurringList title="Out" bills={report.bills.filter((b) => b.amount <= 0)} />
          </>
        )}
      </section>

      {report.regime_changes.length > 0 && (
        <section className="section">
          <div className="label">Regime changes</div>
          <ul className="list">
            {report.regime_changes.map((c) => (
              <li key={`${c.date}-${c.channel}-${c.shift}`}>
                <span>
                  {c.channel === "spend" ? "Spend rate" : "Paycheck"}
                  <span className="sub">
                    {longDate(c.date)}
                    {c.settled ? "" : " · provisional"}
                  </span>
                </span>
                <span className="num">
                  {signedMoney(c.shift)}
                  {c.channel === "spend" ? "/day" : ""}
                </span>
              </li>
            ))}
          </ul>
          <p className="dim" style={{ fontSize: 12, marginBottom: 0 }}>
            The cone widens until the filter relearns the new level.
          </p>
        </section>
      )}
    </aside>
  );
}
