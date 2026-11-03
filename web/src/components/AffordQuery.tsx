// "Can I afford $X by date D?" The engine answers by re-running the same
// forecast (same feed, seed and paths, so the same paths as the cone on
// screen) with X as a target, and this component reads the two
// probabilities for day D out of the report.

import { useState } from "react";
import type { FormEvent } from "react";

import { longDate, money, percent } from "../format";
import type { ForecastOptions, ForecastReport, IsoDate } from "../schema.gen";
import type { SavedQuery } from "../sync/host";

export type RunForecast = (
  feedJson: string,
  options: Partial<ForecastOptions>,
) => Promise<ForecastReport>;

interface Props {
  feedJson: string;
  options: Partial<ForecastOptions>;
  /** The report on screen: it fixes the selectable dates. */
  report: ForecastReport;
  run: RunForecast;
  /** Saved queries, synced between devices, when the shell syncs. */
  saved?: SavedQuery[];
  onSave?: (amountCents: number, by: IsoDate, label: string) => Promise<void>;
  onDelete?: (id: string) => Promise<void>;
}

interface Answer {
  amount: number;
  date: IsoDate;
  onDay: number;
  reachedBy: number;
}

export function AffordQuery({ feedJson, options, report, run, saved, onSave, onDelete }: Props) {
  const days = report.forecast.days;
  const first = days[0]?.date ?? report.as_of;
  const last = days[days.length - 1]?.date ?? report.as_of;
  const [amount, setAmount] = useState("2000");
  const [date, setDate] = useState<IsoDate>(days[Math.min(29, days.length - 1)]?.date ?? first);
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [label, setLabel] = useState("");

  const save = async () => {
    const x = Number(amount);
    if (!Number.isFinite(x) || !onSave) {
      setError("Enter an amount in dollars.");
      return;
    }
    try {
      await onSave(Math.round(x * 100), date, label.trim());
      setLabel("");
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const x = Number(amount);
    if (!Number.isFinite(x)) {
      setError("Enter an amount in dollars.");
      return;
    }
    setBusy(true);
    try {
      const r = await run(feedJson, { ...options, targets: [x] });
      const i = r.forecast.days.findIndex((d) => d.date === date);
      const t = r.forecast.targets[0];
      if (i < 0 || !t) {
        setError(`Pick a date from ${longDate(first)} to ${longDate(last)}.`);
        return;
      }
      setError(null);
      setAnswer({
        amount: x,
        date,
        onDay: t.at_least_on_day[i] ?? 0,
        reachedBy: t.reached_by[i] ?? 0,
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="afford" aria-label="Can I afford it">
      <div className="label">Can I afford it?</div>
      <form onSubmit={(e) => void submit(e)}>
        <label>
          <span className="dim">Have at least $ </span>
          <input
            type="number"
            inputMode="decimal"
            step="any"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
            aria-label="Amount"
          />
        </label>
        <label>
          <span className="dim"> date </span>
          <input
            type="date"
            min={first}
            max={last}
            value={date}
            onChange={(e) => setDate(e.target.value)}
            aria-label="Date"
          />
        </label>
        <button type="submit" className="primary" disabled={busy}>
          {busy ? "Checking…" : "Check"}
        </button>
        {onSave && (
          <>
            <input
              type="text"
              aria-label="Label"
              placeholder="label (optional)"
              value={label}
              maxLength={200}
              onChange={(e) => setLabel(e.target.value)}
            />
            <button type="button" onClick={() => void save()}>
              Save
            </button>
          </>
        )}
      </form>
      {saved && saved.length > 0 && (
        <ul className="saved-queries" aria-label="Saved queries">
          {saved.map((q) => (
            <li key={q.id}>
              <button
                type="button"
                className="link"
                onClick={() => {
                  setAmount(String(q.amountCents / 100));
                  setDate(q.by);
                }}
              >
                {money(q.amountCents / 100)} by {longDate(q.by)}
                {q.label && ` · ${q.label}`}
              </button>
              {onDelete && (
                <button
                  type="button"
                  aria-label={`Delete saved query ${q.label || q.id}`}
                  onClick={() => void onDelete(q.id)}
                >
                  ×
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {error && (
        <p className="dim" role="alert" style={{ color: "var(--bad)" }}>
          {error}
        </p>
      )}
      {answer && !error && (
        <div className="answer" data-testid="afford-answer">
          <div className="answer-grid">
            <div className="answer-card" data-testid="afford-on-day">
              <div className="label">On {longDate(answer.date)}</div>
              <div className="big num">{percent(answer.onDay)}</div>
              <div className="dim">
                chance of at least {money(answer.amount)} at the end of that day
              </div>
            </div>
            <div className="answer-card" data-testid="afford-by-day">
              <div className="label">At any point through {longDate(answer.date)}</div>
              <div className="big num">{percent(answer.reachedBy)}</div>
              <div className="dim">
                chance of at least {money(answer.amount)} at the end of some day from today
                through then
              </div>
            </div>
          </div>
          <p className="why dim">
            Why they differ: the balance swings, so a paycheck can lift it past{" "}
            {money(answer.amount)} for a few days before bills pull it back under; the
            second number is never the lower one.
          </p>
        </div>
      )}
    </section>
  );
}
