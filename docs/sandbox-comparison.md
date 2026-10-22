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

The first run exposed four problems (see "Where the synthetic assumptions
broke" below). They were fixed, and the same raw files were run again, not
refetched, so the before/after numbers compare like with like.

## What the item contains

Seven accounts with 392 transactions. Phase 1 ingests depository accounts
only, so the feed is the **checking account: 91 records, 2 pending**, from
2026-04-21 to 2026-10-21. The credit card (64 records) and five loans (237)
are skipped.

## Before and after the fixes

| | Before | After |
|---|---|---|
| Tagging: income / scheduled / discretionary / transfer | 27 / 14 / 45 / 5 | 27 / 14 / 50 / 0 |
| Why the payroll is income | "payroll" in its name | the largest regular inflow (weekly, fit cost 0) |
| Venmo, Cash App ×2, PayPal, cash deposit | transfers, held, then one-off known flows | spending (no counterpart leg in a linked account) |
| Regime changes detected | none (no detector) | spend rate +$19.51/day on 09-24, +$54.35/day on 09-27, −$97.64/day on 10-15 |
| Spend rate `c` | $18.76/day, ±2σ $14.12 to $23.39 | **$67.04/day, ±2σ −$5.04 to $139.11** |
| Balance today | $470.61, ±2σ $446.46 to $494.77 | $421.38, ±2σ $367.01 to $475.76 |
| 30-day balance, 5% / 50% / 95% | $2,783 / $3,117 / $3,379 | −$492 / $1,595 / $3,761 |
| 90-day balance, 5% / 50% / 95% | $8,116 / $9,144 / $10,116 | −$914 / $4,893 / $10,635 |
| Recurring streams matched to Plaid | 3 of 3 | 3 of 3 (unchanged) |

What actually happened in the data, for reference: discretionary spending
starts on 2026-09-22. With the new tagging it is **$124.02/day gross**
over the 30 days to 10-21 ($104.32/day net of the $95 cash advance and the
$496 cash deposit). Most of that is in the first two weeks: **from 10-07
to 10-21 gross spending is $49.99/day** ($16.93/day net of the deposit). So
there are two changes, a jump in late September and a drop in mid-October,
and the detector found both.

**How to read it.**

- **Before**, the forecast was confident and wrong. It said $18.76 ± 2.32 a
  day when the filter's own input showed six times that, and its cone
  climbed to a tight ~$9.1k at 90 days.
- **After**, the estimate sits between the two recent regimes, with a
  standard deviation of $36/day. The cone is wide accordingly: the 90-day
  interval runs from below zero to $10.6k, around a median of $4.9k. That
  is roughly +$810/week in and 7 × $67 + Spotify out. With one month of
  spending history that has already changed twice, this is the honest
  answer. It narrows as the new regime accumulates days.
- **Which fix did it.** On the same new feed with regime detection turned
  off, `c` is $17.94 ± 2.38 and the 90-day median is $9.2k. The tagging fixes
  are right but don't touch the stale spend rate. The detector does.
- The balance today moved closer to the bank's available balance ($420.23
  = $500 − $79.77 pending). The tail predicts today from the checkpoint with
  `c`, so a stale `c` had pulled it up.

## Detected schedules vs `/transactions/recurring/get`

| Result | Stream | Plaid | Ours | Amount (Plaid / ours) | Next (Plaid / ours) |
|---|---|---|---|---|---|
| match | Sweetgreen (payroll) | WEEKLY (MATURE) | weekly | 810.00 / 810.00 | 2026-10-27 / 2026-10-27 |
| match | Interest payment | MONTHLY (MATURE) | monthly on day 21 | 0.12 / 0.12 | 2026-11-21 / 2026-11-21 |
| match | Spotify | MONTHLY (MATURE) | monthly on day 21 | -12.65 / -12.65 | 2026-11-21 / 2026-11-21 |

- **Matches: 3 of 3** in-scope streams, with the same frequency, amount and
  next date, before and after.
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
streams that the adapter has tagged as income or scheduled, and the adapter
now tags by regularity too. Plaid's recurring engine probably keys on much
the same regularity, so agreement on such clean data is expected rather
than impressive.

## Where the synthetic assumptions broke, and what changed

1. **Spending starts abruptly.** *Fixed by regime detection (design §6).*
   Discretionary spending exists only from 2026-09-22; the five months
   before contain just payroll, interest and Spotify. The random walk on `c`
   (q_c = 0.25) could not follow a step that size, and nothing widened the
   cone. The generator now produces step changes (`SynthConfig::spend_steps`
   and `income_steps`). A CUSUM on the normalized balance innovations now
   triggers a covariance jump in `c` (and, on the income channel, in `m`).
   The three alarms above are that mechanism on this feed.
2. **Balances don't match the history.** *Unchanged.* Sandbox balances are
   static (current $500) whatever the transactions say. The derived opening
   balance is −$18,232.60, so the balance history the filter reconstructs is
   fictional. Only today's level is real.
3. **The payroll is weekly and perfectly flat.** *Unchanged.* It's $810
   every Tuesday with zero variance, so `R_inc` is learned from zero-variance
   data. The generator assumes biweekly pay with N(m, 50²) amounts.
4. **Spending is lumpy.** *Partly addressed.* In one month: an IRS payment
   of $744, Target at $595, a $95 cash advance coming *in* (Brigit) and a
   $496 cash deposit. The gate absorbs these as lumps. The detector clips
   them before accumulating, so one lump cannot set off an alarm. The
   generator still doesn't produce tax payments or cash-advance inflows.
5. **Transfers weren't own-account transfers.** *Fixed.* Every `TRANSFER_*`
   record here is peer-to-peer or cash: Venmo, Cash App ×2 and PayPal out
   ($273), and a $496 cash deposit in. The adapter used to tag them all as
   transfers. They were held out of `z` for 3 business days, blocking
   settlement, then became one-off known flows, so P2P spending never
   reached the spend rate. Now a record is a transfer only when its
   counterpart leg is in another linked depository account: the opposite
   amount to the cent, within 3 business days. This item has only one
   depository account, so all five are spending. In the core, a transfer
   leg that ages out unmatched is also spending now, not a known flow.
6. **No pending→posted links.** *Unchanged.* None of the 15 pending records
   in the item has a posted successor with a `pending_transaction_id`, so
   linking is tested only with fixtures.

## What the adapter decides, and how

- **Income** is the primary *regular* inflow, found by the core's schedule
  detector (`schedule::regular_pay_schedule`). Each counterparty's posted
  inflows are fitted against every weekly, biweekly, semi-monthly and
  monthly schedule, and a stream counts as regular when the fit costs at
  most one day per payday (a missed payday costs 10). Of the regular
  streams, the largest by yearly amount is income. The payee name and
  category (payroll, salary, direct deposit, `INCOME_WAGES`) only break a
  tie within 10%. Here, Sweetgreen's $810 weekly wins outright; the name
  never comes into it. The old rule, "payroll" in the description, is gone.
  - Consequence: a single `INCOME_WAGES` deposit, or an irregular gig
    income, is no longer income. It is a discretionary inflow, which the
    gate treats as a lump.
  - Interest ($0.12 on the 21st) is not a pay schedule: June 21 was a
    Sunday, and paydays roll back to business days. It is tagged by the
    bill rule instead, as before.
- **Scheduled flows** need at least 3 posted records with the same
  counterparty and amount, *on a regular interval*
  (`schedule::is_regular_interval`): a median gap of at least 7 days, with
  80% of gaps within max(3 days, 15%) of it. A bill-like category
  (rent, utilities, loan payments, insurance) still makes a single record
  scheduled. A daily $4.50 coffee is no longer a bill: its gaps are 1–3
  days. Here the rule changes nothing: Spotify and Interest are monthly,
  and no other counterparty repeats an amount 3 times.
- **Transfers:** see item 5.
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
