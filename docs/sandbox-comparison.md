# Plaid Sandbox run: schedule detection vs Plaid, and what broke

Snapshot from 2026-10-21. Test user: `user_transactions_dynamic` at First
Platypus Bank (`ins_109508`), with 730 days requested. Raw data is
gitignored. To reproduce (the dynamic user's data is relative to the fetch
date, so the exact numbers will differ):

```sh
scripts/fetch_plaid_sandbox.py
cargo run -q -p ephemeris-cli -- import-plaid --out data/sandbox/feed.json
cargo run -q -p ephemeris-cli -- forecast --input data/sandbox/feed.json --json data/sandbox/report.json
scripts/compare_recurring.py
```

## What the item contains

Seven accounts with 392 transactions. Phase 1 ingests depository accounts
only, so the feed is the **checking account: 91 records, 2 pending**, from
2026-04-21 to 2026-10-21. The credit card (64 records) and five loans (237)
are skipped.

## Detected schedules vs `/transactions/recurring/get`

| Result | Stream | Plaid | Ours | Amount (Plaid / ours) | Next (Plaid / ours) |
|---|---|---|---|---|---|
| match | Sweetgreen (payroll) | WEEKLY (MATURE) | weekly | 810.00 / 810.00 | 2026-10-27 / 2026-10-27 |
| match | Interest payment | MONTHLY (MATURE) | monthly on day 21 | 0.12 / 0.12 | 2026-11-21 / 2026-11-21 |
| match | Spotify | MONTHLY (MATURE) | monthly on day 21 | -12.65 / -12.65 | 2026-11-21 / 2026-11-21 |

- **Matches: 3 of 3** in-scope streams, with the same frequency, amount and next date.
- **Misses: none. False positives: none.**
- **Out of scope:** OpenAI ($20/mo) and Netflix ($19.57/mo) are on the
  credit card, which Phase 1 doesn't ingest.

**How much this proves: little.** Each stream is perfectly regular, with
identical amounts and no jitter. Spotify is charged on the 21st even when
that day is a Sunday (June 21), so the day-of-month detector correctly
chose "no shift". Nothing here exercises a weekend-shifted ACH bill, a
drifting card charge, a biweekly or semi-monthly payroll, or a variable
amount. The unit tests cover those; this sandbox does not.

There is also a caveat about independence. Our detector only *times*
streams that the adapter has tagged as income or scheduled. The adapter
tagged Spotify and Interest because the same counterparty and amount
repeated at least 3 times. Plaid's recurring engine probably keys on much
the same regularity, so agreement on such clean data is expected rather
than impressive.

## Where the synthetic assumptions break

1. **Spending starts abruptly.** Discretionary spending exists only from
   2026-09-22. The five months before contain just payroll, interest and
   Spotify. The filter's spend rate ends at **$18.76/day**, close to the
   full-history average ($18.22), while the last 30 days actually averaged
   **$111.75/day**. The random walk on `c` (q_c = 0.25) can't follow a step
   that size within a month. The generator models steps in `c` (design §5)
   but doesn't generate them yet. As a result, the cone's median climbs
   about $680/week, to ~$9.1k at 90 days, when the recent weekly net is
   about +$28 ($810 in, ~$782 out). **This forecast is not trustworthy.**
   It is the regime-change case, and it argues for implementing synthetic
   `c` steps and testing a faster-adapting `c` (or a change detector)
   against them.
2. **Balances don't match the history.** Sandbox balances are static
   (current $500) whatever the transactions say. The derived opening
   balance is −$18,232.60. Today's estimate ($470.61 ± 24) is fine, but the
   balance history the filter reconstructs is fictional.
3. **The payroll is weekly and perfectly flat.** It's $810 every Tuesday
   with zero variance. The generator assumes biweekly pay with N(m, 50²)
   amounts. Detection handles it, but `R_inc` is learned from zero-variance
   data.
4. **Spending is lumpy.** In one month: an IRS payment of $744, Target at
   $595, and a $95 cash advance coming *in* (Brigit, categorized
   `LOAN_DISBURSEMENTS`). The generator's lognormal purchases don't produce
   tax payments or cash-advance inflows.
5. **Transfers aren't own-account transfers.** Every record that Plaid
   categorizes as `TRANSFER_*` here is a peer-to-peer or cash movement:
   Venmo, Cash App ×2 and PayPal out, $273 in total, plus a $496 cash
   deposit in. None has a counterpart leg. Per the adapter rule they're
   tagged transfer, held out of `z` for up to 3 business days (blocking
   settlement), then reclassified as one-off known flows. So **P2P
   spending never counts as spending** and never enters the forecast's
   spend rate.
6. **No pending→posted links.** None of the 15 pending records in the item
   has a posted successor with a `pending_transaction_id`, so linking is
   tested only with fixtures.

## What the adapter had to guess

- **Income.** The payroll is categorized `FOOD_AND_DRINK_RESTAURANT`
  (merchant "Sweetgreen"). It is tagged income only because its description
  contains "payroll". A payroll without that word, under a non-wage
  category, would be missed. It would then become a scheduled stream
  through the repeat rule, so it would still appear in the forecast, but
  as a bill, without the `m` model.
- **Scheduled flows** come from repetition (same counterparty and amount
  at least 3 times) or a bill-like category, not from anything Plaid
  states. That rule would also tag a habitual $4.50 coffee as a bill.
- **Interest** (+$0.12/mo) is tagged a scheduled *inflow* by the repeat
  rule. That's harmless, and it matches Plaid's view.
- **Transfers:** see item 5 above. The rule follows Plaid's category, but
  our model's "transfer" means own-account. A P2P payment probably belongs
  under discretionary.
- **Ledger balance** = the sum of depository `balances.current`. Here
  current − available = $79.77, exactly the two pending debits, which is
  consistent with "current excludes pending", but that isn't documented
  for every institution.
- **`history_start`** = the earliest transaction (2026-04-21), not the
  730-day window requested. Starting at the window would have fed the
  filter 18 months of observed, unchanging balance and pinned `c` at 0.
- **`today`** = the fetcher's local date.
- **Stream identity** = Plaid `merchant_name`, else the description with
  digits and punctuation removed.
- **Rounding:** Plaid's sandbox reports amounts like 48.2542, rounded to
  cents here.
