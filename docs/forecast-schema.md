# JSON contracts, schema version 1

Two documents cross the engine boundary. Both are defined as serde types in
`crates/ephemeris-core/src/schema.rs` (feature `serde`), and this file
documents them. Change the two together.

| Document | Producer | Consumer |
|---|---|---|
| **Feed** | `ephemeris simulate`; the Plaid adapter (later) | `ephemeris forecast` |
| **Forecast report** | `ephemeris forecast --json` | the Phase 2 web shell |

## Conventions

- `schema_version` is an integer, currently `1`. Readers reject any version
  they don't know. Bump it on any breaking change: a field removed, renamed,
  retyped, or given a different meaning. Adding an optional field is not
  breaking. Readers ignore unknown fields.
- **Dates** are strings in strict `YYYY-MM-DD` form, a real calendar date in
  the account's local calendar, with no time or zone.
- **Money** is a JSON number in account currency, **inflows positive**. Plaid
  uses the opposite sign, and the Plaid adapter flips it. No value is ever
  `NaN` or infinite.
- Optional values are `null`. In a feed they may also be omitted.
- Integer IDs fit in a `u64`. JavaScript loses precision above 2⁵³, so treat
  `id`, `pending_id` and `seed` as opaque if they could be that large.

## Feed

One bank-feed snapshot: every record visible at the end of `today`, across
all linked depository accounts.

```json
{
  "schema_version": 1,
  "history_start": "2024-01-01",
  "today": "2024-12-30",
  "ledger_balance": 4182.37,
  "transactions": [
    {
      "id": 1042,
      "account": 0,
      "date": "2024-12-27",
      "authorized_date": "2024-12-26",
      "status": "posted",
      "amount": -48.19,
      "kind": "discretionary",
      "stream": null,
      "pending_id": 1041,
      "merchant_name": "Target",
      "description": "INTERAC PURCHASE - 67200 TARGET"
    }
  ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `history_start` | date | First day of fetched history. No transaction is dated earlier. |
| `today` | date | The snapshot describes the end of this day. No record is dated later. |
| `ledger_balance` | number | The bank's **posted** balance across linked accounts at the end of `today`. Pending records are not in it. |
| `transactions[]` | array | Every record the feed shows, pending and posted, in any order. |

Each transaction:

| Field | Type | Meaning |
|---|---|---|
| `id` | integer | Unique within the feed. A pending record and the posted record that replaces it have different ids. |
| `account` | integer | Linked account. Own-account transfer legs must be on different accounts. |
| `date` | date | Posted date (posted), or authorization date (pending). Plaid's `date`. |
| `authorized_date` | date or null | When the purchase happened. Not after `date`. When missing, the engine uses `date` and widens its uncertainty. |
| `status` | `"pending"` \| `"posted"` | |
| `amount` | number | Signed, inflows positive. |
| `kind` | `"income"` \| `"scheduled"` \| `"discretionary"` \| `"transfer"` | How the flow enters the model (design §0–§1). `income` is the primary paycheck only. |
| `stream` | integer or null | Recurring stream (an employer or a subscription). Bill detection groups `scheduled` records by it. |
| `pending_id` | integer or null | On a posted record, the pending record it replaces. Must be null on pending records. |
| `merchant_name` | string or null | Display name of the counterparty (Plaid `merchant_name`). For display only; the engine never reads it. |
| `description` | string or null | The bank's description line (Plaid `name`). For display only. |

## Forecast report

```json
{
  "schema_version": 1,
  "as_of": "2024-12-30",
  "settled_through": "2024-12-26",
  "balance":           { "mean": 4170.2, "sd": 31.4, "low_2sd": 4107.4, "high_2sd": 4233.0 },
  "spend_rate":        { "mean": 58.7,   "sd": 4.1,  "low_2sd": 50.5,   "high_2sd": 66.9 },
  "expected_paycheck": { "mean": 2391.0, "sd": 22.8, "low_2sd": 2345.4, "high_2sd": 2436.6 },
  "income": {
    "name": "Sweetgreen",
    "cadence": { "kind": "biweekly", "anchor": "2024-01-05" },
    "last_paid_nominal": "2024-12-20",
    "next_scheduled": "2025-01-03",
    "offset_probabilities": [0.01, 0.15, 0.68, 0.15, 0.01],
    "fit_cost": 9
  },
  "bills": [
    { "stream": 100, "name": "Rent", "amount": -1500.0, "period_days": 31,
      "monthly_on": { "day": 1, "shift": "next_business_day" },
      "last_charged": "2024-12-02", "next_due": "2025-01-02" },
    { "stream": 101, "name": "Netflix", "amount": -15.99, "period_days": 30, "monthly_on": null,
      "last_charged": "2024-12-11", "next_due": "2025-01-10" }
  ],
  "forecast": {
    "horizon_days": 90,
    "paths": 2000,
    "seed": 0,
    "levels": [0.05, 0.25, 0.5, 0.75, 0.95],
    "days": [
      { "date": "2024-12-31", "quantiles": [4012.5, 4081.9, 4118.0, 4155.3, 4220.8] }
    ]
  }
}
```

(Numbers are illustrative.)

| Field | Type | Meaning |
|---|---|---|
| `as_of` | date | End of the day the estimate describes (the feed's `today`). |
| `settled_through` | date | Last day whose records are all final. Later days still hold pending amounts. |
| `balance` | estimate | True balance `b`, including pending-amount uncertainty. |
| `spend_rate` | estimate | Discretionary spend rate `c`, in $/day, **outflow positive**. |
| `expected_paycheck` | estimate | Expected primary paycheck `m`. |
| `income` | object or null | Detected pay schedule. When it's null, the forecast includes no income. |
| `bills[]` | array | Detected recurring bill streams. |
| `forecast` | object | The Monte Carlo cone. |

An **estimate** is `{ mean, sd, low_2sd, high_2sd }`, with `low_2sd = mean − 2·sd` and
`high_2sd = mean + 2·sd`. It describes the filter's Gaussian posterior, not
a forecast.

**`income`:**

| Field | Type | Meaning |
|---|---|---|
| `name` | string or null | The payer's name from the latest paycheck (merchant name, else description). |
| `cadence` | object, tagged by `kind` | `{"kind":"weekly","anchor":date}`, `{"kind":"biweekly","anchor":date}`, `{"kind":"semi_monthly","days":[d1,d2]}`, or `{"kind":"monthly","day":d}`. A day of the month past the end of a month means its last day. |
| `last_paid_nominal` | date | Nominal date of the most recent observed paycheck. |
| `next_scheduled` | date or null | The next nominal date, moved to the previous business day (US Federal Reserve calendar), before timing jitter. |
| `offset_probabilities` | 5 numbers | Probability that a paycheck lands −2, −1, 0, +1 or +2 days from its nominal date, before the business-day shift. They sum to 1. |
| `fit_cost` | integer | How well the cadence explains the history (0 = exactly). |

**`bills[]`:**

| Field | Type | Meaning |
|---|---|---|
| `stream` | integer | The feed's stream id. |
| `name` | string or null | From the stream's latest named record: merchant name, else description. |
| `amount` | number | Latest posted charge, signed. |
| `period_days` | integer | Median gap between charges. |
| `monthly_on` | object or null | Set when the bill is charged on a fixed day of the month: `{ "day": 1–31, "shift": "none" \| "previous_business_day" \| "next_business_day" }`. A day past the end of a month means its last day. `shift` says how a weekend or bank-holiday date moves. When it's null, the bill recurs every `period_days`, with drift. |
| `last_charged` | date | Most recent charge. |
| `next_due` | date | The next charge: by day of month, shift applied, when `monthly_on` is set; otherwise `last_charged + period_days`, before drift. |

**`forecast`:**

| Field | Type | Meaning |
|---|---|---|
| `horizon_days` | integer | Number of forecast days. |
| `paths` | integer | Monte Carlo paths. |
| `seed` | integer | The same feed, seed and paths give the same cone, bit for bit. |
| `levels` | numbers | Quantile levels in (0, 1), ascending. |
| `days[]` | array | One entry per day from `as_of + 1` through `as_of + horizon_days`, in order. `quantiles[i]` is the end-of-day balance at `levels[i]`, non-decreasing in `i`. |

Quantiles are the cone's **marginal** per-day quantiles. A line through the
medians is not a plausible path, and the chance of dipping below a threshold
at some point is not readable from them.
