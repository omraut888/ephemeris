// Reports produced by the real engine, not hand-made, so component tests
// see real shapes.

import { forecast, simulateJson } from "../src/wasm/engine";
import type { ForecastReport } from "../src/schema.gen";

export const FEED_JSON = simulateJson({ seed: 7, days: 240, mode: "realistic" });

export const REPORT: ForecastReport = forecast(FEED_JSON, {
  horizon_days: 60,
  paths: 400,
  seed: 0,
});

/** No income: cycle lows fall back to calendar weeks. */
export function reportWithoutIncome(): ForecastReport {
  const feed = JSON.parse(FEED_JSON) as { transactions: { kind: string }[] };
  for (const t of feed.transactions) {
    if (t.kind === "income") t.kind = "discretionary";
  }
  return forecast(JSON.stringify(feed), { horizon_days: 30, paths: 200 });
}
