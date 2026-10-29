// Reports produced by the real engine, not hand-made, so component tests
// see real shapes.

import { forecast, simulateJson } from "../src/wasm/engine";
import type { ForecastReport } from "../src/schema.gen";

export const FEED_JSON = simulateJson({ seed: 7, days: 240, mode: "realistic" });

export const REPORT: ForecastReport = forecast(FEED_JSON, {
  horizon_days: 60,
  paths: 400,
  seed: 0,
  targets: [0],
});

/** The demo feed with $21,000 less in the bank: the cone crosses $0. */
export const LOW_FEED_JSON = (() => {
  const feed = JSON.parse(FEED_JSON) as { ledger_balance: number };
  feed.ledger_balance -= 21000;
  return JSON.stringify(feed);
})();

export const LOW_REPORT: ForecastReport = forecast(LOW_FEED_JSON, {
  horizon_days: 60,
  paths: 400,
  seed: 0,
  targets: [0],
});

/** No income: cycle lows fall back to calendar weeks. */
export function reportWithoutIncome(): ForecastReport {
  const feed = JSON.parse(FEED_JSON) as { transactions: { kind: string }[] };
  for (const t of feed.transactions) {
    if (t.kind === "income") t.kind = "discretionary";
  }
  return forecast(JSON.stringify(feed), { horizon_days: 30, paths: 200 });
}
