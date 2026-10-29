# Phase 1 design: `ephemeris-core`

Status: proposal, not implemented yet. Scope: the Kalman state estimator, the
Monte Carlo forecaster, and a synthetic validation harness. Everything here is
pure computation (see CLAUDE.md). Plaid ingestion lives in `ephemeris-cli`.

## 0. Conventions

- **Time:** `Day = i32`, counting days since 1970-01-01 in the account's local
  calendar. The core never handles dates or time zones; the CLI converts them.
  This also keeps `chrono`/`time` out of the core.
- **Money:** `f64` in account currency. **Inflows are positive.** Plaid's
  convention is the opposite (positive means money out), so the Plaid adapter
  flips the sign. The core never sees Plaid's convention.
- **Day assignment:** a transaction belongs to its `authorized_date`. If that
  is missing, it falls back to the posted `date` (see §2). Banks don't post on
  weekends, so grouping by posted date would make Saturday look like a
  zero-spend day and Monday look like a spike.
- **Scope:** Phase 1 covers depository accounts only (checking and savings),
  aggregated into one cash position (`b` is the sum across accounts). Credit
  cards come later.
- **Own-account transfers:** every `Transaction` carries an `AccountId`. A
  movement between two of the user's linked accounts is tagged
  `FlowKind::Transfer`. A record is a transfer only because its counterpart
  leg is in another linked account, never because of its category: a Venmo,
  Cash App, PayPal or cash movement with no such leg is spending. Transfers
  are never spending or income:
  - They are excluded from `u`, from the spending rate `c` and from the income
    observations of `m`.
  - Their two legs cancel exactly in the aggregated balance.
  - A transfer to or from an *external* account (a friend, a brokerage that
    isn't linked) is not a `Transfer`; it is an ordinary flow.
  - The two legs can become visible on different days. §2 covers how the
    balance observation handles a leg whose counterpart hasn't appeared yet.

## 1. State and models

### State

```
x_k = [ b_k, c_k, m_k ]ᵀ
  b  true disposable balance, end of day k            ($)
  c  baseline discretionary spend rate                ($/day, outflow > 0)
  m  expected amount of the primary income event      ($/event)
```

How this maps to the spec's three state variables:

- *True disposable balance* is `b`.
- *Recurring-expense baseline* is split into two parts:
  - **Scheduled** recurring flows (rent, subscriptions, loan payments) are
    discrete and have known dates. They enter as the control input `u`, and a
    recurrence detector (§4, `schedule`) gives them to the forecaster.
  - The **smooth** part, everyday spending, is `c`.

  A filter that blends rent into a per-day rate gives a badly shaped cone.
- *Income regularity variance* is not a state. It becomes an adaptively
  estimated noise parameter (§3).

### Dynamics (Δt = 1 day)

```
b_k = b_{k-1} - c_{k-1} + u_k + w_b      u_k = Σ realized scheduled flows on day k
c_k = c_{k-1}             + w_c          (income +, recurring bills −)
m_k = m_{k-1}             + w_m

A = | 1 -1  0 |     B = | 1 |     Q = diag(σ_s², q_c, q_m)
    | 0  1  0 |         | 0 |
    | 0  0  1 |         | 0 |
```

- `σ_s²` is the day-to-day spread of discretionary spending. It drives the
  width of the cone, so it is estimated adaptively (§3).
- `q_c` and `q_m` are random-walk drift rates (fixed tuning parameters, chosen
  using the synthetic harness).
- The three noise sources are independent, so Q is diagonal.

### Paychecks are a known input, not an innovation

On the day a paycheck posts, its actual amount `a` takes two separate paths.

1. **Into `b` as a control input.** `u_k` includes `+a`. The predict step
   gives `b⁻_k = b_{k-1} − c_{k-1} + a`, so the predicted balance already
   includes the paycheck. When the balance observation arrives, the innovation
   `z − b⁻` contains only the deviation of discretionary spending and any
   unmodeled flows, not the $2,400 jump.
2. **Into `m` as an observation.** A separate scalar update with `H = [0 0 1]`
   and `z = a` (`R̂_inc`, χ²-gated). This is the only place the paycheck
   amount teaches the filter anything.

These two paths don't double-count. `A` has no `m` column, so `m` never moves
`b` inside the filter. `b` gets the realized amount through `u`, and `m` learns
from the same number through its own channel. Scheduled bills follow path 1
only.

The filter never predicts *when* a paycheck lands. If it did, a paycheck
arriving a day late would show up as a −m innovation followed by a +m
innovation, which would corrupt both `b` and `m`. Predicting income timing is
the forecaster's job (§4).

**Invariant (enforced in `observe`):** every flow included in a day's balance
observation `z` is either in that day's `u` (income, scheduled) or modeled by
`−c` (discretionary). Transfers net to zero and appear in neither. If the
invariant breaks, the adaptive spending-variance estimator (§3) reads the
mismatch as volatility. The typical case is a paycheck from a new employer that
isn't tagged `Income`.

**Guard:** balance innovations are χ²-gated too, at the χ²₁ 99.9% point
(NIS > 10.83). For a gated innovation, only the *excess* beyond the gate is
treated as an unexplained lump flow: `ν − sign(ν)·√(gate·S)` is added to `b`
(equivalent to adding it to `u` and re-predicting), and the normal update then
runs on the clipped remainder (a Huber-style bounded influence). The result:
- `b` still tracks the bank.
- The lump moves `c` only as much as a 3.29σ day would, not by its full size.
- The variance estimator sees the clipped sample, not the raw one.
- It is reported in `DayReport::lump` so the adapter's tagging can be fixed.

A gated *income* observation skips the `m` update entirely (a bonus). Its
clipped sample still reaches the `R_inc` estimator; see §3 for why.

**Measured cost on heavy-tailed spending.** In the realistic generator 1.28%
of days are beyond 3.29σ (0.1% for a Gaussian). Those days' excess goes to
lump flows, so the adaptive `σ̂_s²` measures the *clipped* spread: about 2650
against a true daily variance of 3711. The forecaster must add the lump-flow
distribution back (for example, bootstrap the reported lumps), or the cone will
be too narrow on real spending.

### Pay schedules

The supported schedules (`calendar::PaySchedule`) are:
- `Weekly { anchor }`
- `Biweekly { anchor }`
- `SemiMonthly { days: [d1, d2] }`, for example the 1st and 15th, or the 15th
  and 31st
- `Monthly { day }`

A day of the month past the end of a month means its last day. "Last business
day" is not supported.

Nominal dates move to the **previous business day** under the US Federal
Reserve holiday calendar. A fixed-date holiday on a Sunday is observed Monday;
one on a Saturday is not observed. For example, a 15th that falls on Saturday
pays Friday the 14th.

These shifts are deterministic. The forecaster applies the same rule to future
dates, so the only residual it has to model as random is processing jitter,
such as early direct deposit. Posting lags use the same business-day calendar.

### Day-of-week effects

Day-of-week effects are deferred in the model: `c` is a weekly-average rate
with no weekday structure. The realistic generator keeps a weekly pattern, so
we can measure what ignoring it costs. The spending innovations will have
weekly autocorrelation, which shows up in NIS.

### Observations

Measurements are processed **one scalar at a time**. This is equivalent to a
joint update because R is diagonal. It needs no matrix inverse and handles a
different set of observations each day without extra code.

| Channel | H | z | R |
|---|---|---|---|
| Balance | `[1 0 0]` | end-of-day balance on the authorized basis | `r_post` (≈ $1², absorbs fees, interest, rounding) plus pending terms (§2) |
| Income | `[0 0 1]` | amount of a posted primary-income transaction | `R̂_inc`, adaptive (§3) |

Income observations are **gated**: an income event with NIS above a χ²₁
threshold (for example, a bonus) is applied as a control flow but is not used
as an observation of `m`.

### Predict-only days

A day with no observations runs a predict step and nothing else. One point to
get right: **a day with no transactions is not a day with no observation.** If
the transaction history is complete, the balance on that day is known; it's just
unchanged.

Because the balance channel observes a *level*, not a flow, skipping those days
introduces no bias. The next observed level contains the total change, so we
only lose a little variance reduction. If we observed daily *flows*, skipping
zero-flow days would bias `c` upward. That is the main reason to use level
observations.

Days that really are predict-only:
- gaps in the data (sync failures, days before the account was linked)
- days in the provisional tail (§2)

### Numerics

- Joseph-form covariance update, then `P ← (P + Pᵀ)/2`.
- `debug_assert!` that P is PSD after every step: `P + tol·I` must have a
  Cholesky factorization, with tol relative to the largest entry.
  Tests check the same thing explicitly.
- Prior: initialize `b` from the first observation. Initialize `c` and `m` with
  wide variances (for example, `σ_c = $100/day`).
- Hand-computable check for the predict step, with P = diag(p_b, p_c) and the
  2-state block:
  `P⁻ = [[p_b + p_c + σ_s², −p_c], [−p_c, p_c + q_c]]`.
  The off-diagonal term is how balance observations end up teaching the filter
  about `c`.

## 2. Pending vs. posted

The history is split at `settled_through`, the day before the earliest
authorized date of any transaction that is still pending.

- **Settled history** (everything ≤ `settled_through`): all transactions are
  posted with final amounts. The filter runs over these days once and saves a
  **checkpoint** `(x̂, P, adaptive state)`.
- **Provisional tail** (`settled_through` + 1 through today): on each sync, the
  tracker is cloned from the checkpoint and the tail is replayed. The tail runs
  predict-only on every day, plus one balance observation at today. The
  `u` for each tail day is the scheduled and income flows visible at today:
  posted ones at their final amount, pending ones at their pending amount.
  The balance observation is:

  ```
  z_today = posted_ledger + Σ_j a_j                (a_j signed pending amounts)
  R_today = r_post + Σ_j σ_j²,   σ_j = max(ρ·|a_j|, σ_min)
  ```

  - `ρ` (≈ 0.2) covers tips and amount corrections.
  - `σ_min` covers hold-type authorizations, such as a $1 gas pre-auth that
    settles at $60.
  - Phase 1 uses one `(ρ, σ_min)` pair. Values per category are an easy
    extension once categories exist.

**Pending scheduled flows share their error between `u` and `z`.** A pending
rent charge enters both the tail's `u` and `z_today` at its pending amount, so
its error `e` (σ_j as above) appears in the prediction *and* the measurement and
cancels in the innovation. Treating `e` as process noise (extra `Q_bb`) would
let the observation appear to remove an error it doesn't contain, and the
filter would become overconfident. The synthetic test measured this: 2σ
coverage of `b` was 86.6% instead of about 95%.

The correct handling: the tail filter estimates the balance *as shown*, which
is the true balance plus `e`, with consistent covariance. The tracker then
reports `P_bb + Σ σ_j²` over pending scheduled flows. `c` and `m` are
unaffected. Pending *discretionary* purchases are different: they are not in
`u`, so their error is ordinary measurement noise in `R_today`.

Pending transactions are never part of the checkpoint. When one posts (Plaid
links it through `pending_transaction_id`), it moves into settled history at its
final amount, and the checkpoint advances. This avoids the main problem with
treating pending as a noisier observation on every day: one wrong pending amount
would corrupt every later day's level observation, so the errors would be
correlated across days and R would no longer be white. With this design the
uncertainty appears exactly once, at today, and goes away when the transaction
posts.

**Known gap:** a pending authorization that is reversed and never posts is a
bias, not variance. We accept this. The next sync corrects it.

**Transfers in flight.** The outgoing leg of an own-account transfer usually
shows as pending at once. The incoming leg often appears only when it posts,
0–1 business days later. On the authorized basis both legs belong to the same
day and cancel. Until the second leg is visible, though, a naive `z` would
show a spurious −amount.

`observe` therefore holds an unmatched `Transfer` leg out of `z` until its
counterpart appears. Matching is by opposite amount within a few business days.
If no counterpart appears within N business days, the leg is reclassified as
spending (seen through the balance, like a discretionary purchase). The generator produces exactly this
visibility pattern.

Held legs **block settlement** (`settled_through` stops before them), so a
later reclassification never rewrites history that's already settled.

**Anchoring and rebuilds.** `observe` takes the bank's posted balance across
the linked accounts and derives the opening balance as `ledger − Σ posted
amounts`. Settled-day observations are `opening + Σ posted counted amounts with
day ≤ d`. On each ingest the tracker recomputes the inputs of every settled day
it has already consumed, comparing to within $10⁻⁶ because summation order can
change between snapshots. If any day changed, or the opening balance did, it
rebuilds from the prior. Typical causes are a backdated record, or a fee with
no transaction. Today a fee with no transaction is spread back to the opening
balance; recording it as a dated adjustment is a possible refinement.

A posted transaction with no `authorized_date` gets its posted date. Its R
contribution is inflated by `(ρ_lag·|a|)²`, because it may belong to an earlier
day.

## 3. Income regularity variance

Making a variance part of the state turns the noise model multiplicative. That
would require an EKF, a UKF, or a particle filter. It also leaves the variance
poorly identified, because there are only about 26 paychecks a year, and we
would lose the closed-form, hand-testable updates that CLAUDE.md asks for.

**Proposal:** estimate the noise variances from the filter's own innovations
(covariance matching, in the style of Mehra/Myers–Tapley), with an explicit
count of effective samples.

Each channel has exactly one unknown variance, so each is identifiable:

- **Income channel, unknown `R_inc`.** For an income innovation `ν` with
  predicted variance `s⁻ = H P⁻ Hᵀ` (excluding R):

  ```
  n   ← λ·n + 1
  R̂  ← R̂ + ((ν² − s⁻) − R̂) / n
  R̂  ← clamp(R̂, R_min, R_max)
  ```

  It starts at `(R̂, n) = (R_prior, n₀)`. The pseudo-count `n₀` works like an
  inverse-gamma prior, and `λ` (for example, 0.97 per event) lets the estimate
  follow a job change.

- **Balance channel, unknown `σ_s²`.** `r_post` is known and small. For an
  observation after a gap of `g` days:

  ```
  sample = (ν² − (P⁻_bb − g·σ̂_s²) − R) / g
  ```

  This is fed into the same estimator. Dividing by `g` handles irregular
  observation spacing correctly.

**Implementation details (`adaptive`, `tracker`):**

- **Weights.** Samples are weighted by their information, `w = (v/S)²`, where
  `v` is the variance's share of `S`. Without the weight, the first paycheck
  under a diffuse `m` prior gives `ν² − s⁻ ≈ −2·10⁷` and slams `R̂` to its
  floor. The effective count is `n ← λ·n + w`, so dof stays honest.
- **Lock-out escape.** Plugging `R̂` in for `v` would freeze a prior that is far
  too small: every `S` looks huge next to it, so every weight is about 0. The
  plug-in is therefore `v = max(R̂, sample/10)`. Measured over 50 seeds: a
  divisor of 4 biased `R̂_inc` +18%, 10 biased it +5%, and no escape hatch
  (+1%) left a `R̂ = 1` prior stuck at 1.3.
- **Gated samples are clipped, not dropped.** Dropping them is the other
  lock-out route: with `R̂` too small, every real paycheck is gated, and gated
  samples would never raise `R̂`.
- **Adaptation runs only on settled days.** The provisional tail is replayed
  on every sync and would otherwise be counted several times.
- **Defaults:**

  | Variance | Prior | Pseudo-count n₀ | λ | Clamps |
  |---|---|---|---|---|
  | `σ_s²` | 50² | 4 | 0.995 per day | [5², 10⁶] |
  | `R_inc` | 100² | 2 | 0.97 per event | [1, 10⁸] |

**What the forecaster gets:** `VarianceEstimate { value, dof: n }`. The MC
draws each path's variance from a scaled-inverse-χ²(n, R̂) distribution
instead of using the point estimate. With only 4 paychecks of history, the cone
gets wider (effectively a Student-t), which is the honest result.

**Tradeoffs:**

- **Pros**
  - Stays linear-Gaussian, so every update can be checked by hand.
  - About 20 lines of code.
  - Adapts when the regime changes.
  - Reports how uncertain the variance estimate itself is.
- **Cons**
  1. *Small samples:* the estimate is noisy until n is around 10. The prior and
     the dof handling soften this but don't remove it.
  2. *Feedback:* R̂ changes the gain, the gain changes the innovations, and the
     innovations change R̂. The clamps and a slow λ keep this stable, and we
     test convergence on synthetic data.
  3. *Outliers:* one bonus inflates R̂. The χ² gate excludes it. The cost is
     that gating can hide variability that is real, as with gig income.
  4. *Timing is not covered.* Payday jitter is a timing quantity, which a
     linear KF can't represent. `schedule` estimates it separately from
     inter-arrival times.
- **Alternatives rejected for now**
  - Augmented state with a UKF or particle filter: nonlinear and weakly
    identified.
  - VB adaptive KF (Särkkä–Nummenmaa): the principled upgrade, and the
    `{value, dof}` interface already matches it. We switch if the Phase 6
    calibration shows the cone is overconfident on short histories.
  - Fixed, hand-tuned values: can't become calibrated for each user.

## 4. Crate layout, API, and dependencies

```
crates/ephemeris-core/src/
  lib.rs        #![forbid(unsafe_code)], re-exports
  error.rs      EngineError
  types.rs      Day, AccountId, Transaction (mirrors Plaid: account, date, authorized_date,
                status, amount, kind: Income|Scheduled|Discretionary|Transfer, stream,
                pending_id; day() = authorized date or posted date),
                Observation { channel, z, r }, DayInput { day, u, observations }
  calendar.rs   civil dates, Fed holiday calendar, business days, PaySchedule
  linalg.rs     symmetrize, is_psd, chol_with_jitter
  kalman.rs     generic Kf<const N>: predict(A,B,u,Q), update_scalar(h,r,z) -> Innovation
  model.rs      the 3-state finance model: A, B, H rows, Q builder, ModelParams
  adaptive.rs   VarianceEstimator, AdaptiveConfig (incl. ::fixed), sample_weight (§3)
  change.rs     ChangeDetector: two-sided CUSUM on normalized innovations (§6)
  observe.rs    ObservedFeed::new(snapshot, ledger_balance, today): roles (incl. transfer
                matching), settled_through, opening balance, FeedDay { u, balance: Option<(z,R)>,
                income: Vec<f64> } for settled days and the tail, shared_variance
  tracker.rs    Tracker::ingest(snapshot, ledger_balance, today): checkpoint + provisional
                tail, gating, adaptive noise; rebuilds from the prior if consumed settled
                days change; returns DayReport per day
  schedule.rs   detect_schedule(snapshot) -> Schedule { income: PayTiming, bills: Vec<BillStream> }
  forecast.rs   forecast(estimate, schedule, model, cfg) -> Cone (bands, P(b ≥ X))
  synth.rs      seeded generator with ground truth (§5)
  metrics.rs    CRPS (Gaussian, quantile grid, point); NEES, NIS, RMSE, interval
                coverage to follow (shared by tests and CLI)
```

`synth` and `metrics` are ordinary pure modules. If a binary doesn't use them,
the linker drops them, so a feature gate isn't needed unless they grow
dependencies. The core does no printing. Diagnostics are returned as data in
`StepReport { day, innovations: Vec<Innovation{nu, s, nis}> }`.

### Public API sketch

```rust
pub struct Tracker { .. }                     // Clone
impl Tracker {
    pub fn new(params: ModelParams, prior: Prior) -> Self;
    pub fn ingest(&mut self, txns: &[Transaction], today: Day)
        -> Result<Vec<StepReport>, EngineError>;
    pub fn estimate(&self) -> StateEstimate;  // provisional (includes the tail)
    pub fn checkpoint(&self) -> StateEstimate; // settled only
}
pub struct StateEstimate { pub day: Day, pub x: Vector3<f64>, pub p: Matrix3<f64>,
                           pub spend_var: VarianceEstimate, pub income_var: VarianceEstimate }

pub fn detect_schedule(txns: &[Transaction]) -> Schedule;
pub struct ForecastConfig { pub horizon_days: u32, pub n_paths: u32, pub seed: u64,
                            pub thresholds: Vec<f64> }
pub fn forecast(est: &StateEstimate, sched: &Schedule, cfg: &ForecastConfig)
    -> Result<Cone, EngineError>;
impl Cone {
    pub fn quantile(&self, day: Day, q: f64) -> f64;      // stored on a 1..99% grid
    pub fn prob_at_least(&self, day: Day, amount: f64) -> f64;
    pub fn prob_breach_by(&self, day: Day, threshold_idx: usize) -> f64; // first passage
}
```

`kalman::Kf<N>` is also public, so tests can work through hand-computed 1-state
and 2-state cases, and Phase 6 baselines can reuse it. Invalid input (NaN, a
non-positive S, a negative variance) returns `EngineError`. The library never
panics on bad input.

### Monte Carlo design

- **Paths:** each path draws `x₀ ~ N(x̂, P)` using `chol_with_jitter`, and draws
  its own `σ_s²` and `R_inc` from the scaled-inverse-χ² distributions.
- **Each simulated day:**
  - `c` and `m` take a random-walk step.
  - `b += −c + N(0, σ_s²)`.
  - Scheduled events fire on dates with jitter added. Income is drawn as
    `N(m, R_inc)`.
- **Simulation order:** day-major. The engine keeps N live states and computes
  the day's quantiles with `select_nth_unstable`.
- **Memory:** O(N + H·99), not O(N·H), which matters on mobile and in WASM.
- **Seeding:** every path gets its own RNG stream:
  `ChaCha8Rng::seed_from_u64(seed)` followed by `set_stream(path_idx)`. Path
  *i* is therefore identical whatever N is and whatever the loop order, so
  results stay reproducible after parallelization.
- **Seed:** required. The API has no entropy-based constructor, so the
  reproducibility rule in CLAUDE.md holds by construction.

### Monte Carlo as built

- **RNG: counter-based, not N live generators.** Path `i` on step `h` uses
  its own ChaCha8 segment (`stream = i`, `word_pos = h·2²⁴`). Memory is O(N)
  small path states, about 60 bytes each, with no per-path generator kept
  alive. Path `i` is identical for any N and any loop order, which a test
  checks.
- **Variance draws.** dof is floored at 1, and each draw is capped at 100× the
  estimate so a very thin history cannot produce absurd paths.
- **Paychecks:** detected nominal dates after `last_paid_nominal`, plus an
  offset sampled from the fitted distribution, moved to the previous business
  day. A paycheck whose sampled day has passed unobserved arrives on the next
  business day. Without a detected schedule, the forecast has no income.
- **Bills:** each detected stream recurs at `period + drift`, at its latest
  posted amount.
- **Lumps:** bootstrap, not a parametric tail. The tracker keeps every lump
  its gate absorbed on settled days. Each simulated day:
  - A lump occurs with rate `(n + 0.001·1000)/(days + 1000)`.
  - Its size is one of the observed lumps with probability `n/(n + 5)`.
  - Otherwise its size comes from the prior: the Gaussian excess beyond the
    gate, `±σ̂_s·Exp(3.29)`.

  So with no history the cone gets back exactly what the gate removed under
  the Gaussian model, and with history the observed lumps take over.

  **Why not a heavy-tailed distribution (Student-t, lognormal, Pareto)?**
  1. The tail index is the hardest parameter to estimate, and a few lumps a
     year can't pin it down. A wrong index gives either a cone that is too
     narrow or one with infinite variance.
  2. Real lumps are asymmetric: big purchases, refunds, bonuses. The bootstrap
     keeps their sign mix and sizes without a shape assumption.
  3. It adds back exactly the part the gate removed, so the core Gaussian
     plus the lumps reconstructs the observed day distribution.

  The cost: with a short history the bootstrap can't produce a lump bigger
  than the largest seen, so it understates the extreme tail. The prior
  component and its weight soften this. A smoothed bootstrap (jittered
  resamples) is the next step if calibration shows it.
- **Outputs:** per-day quantiles at configurable levels (default
  5/25/50/75/95%), `P(b_D ≥ X)`, `P(b ≥ X on some day ≤ D)` and
  `P(b < X on some day ≤ D)` for each configured target X.

### Recurrence detection (`schedule.rs`)

**Income.** Every `PaySchedule` candidate is scored against the last 26
observed paydays (weekly and biweekly at every anchor phase, monthly on every
day, semi-monthly on every pair of days).
- Each payday costs the smallest `|k| ≤ 2` with `pay_date(nominal + k) =
  payday`, or a miss penalty of 10 if no such `k` exists.
- Each nominal date inside the observed span with no paycheck also costs 10,
  which keeps weekly from beating biweekly.
- The cheapest candidate wins. The fitted offsets become an empirical
  distribution over −2..+2, smoothed toward a prior centred on 0
  (2 pseudo-observations).

**Bills.** One entry per `Scheduled` stream with at least 2 charges: period =
median gap, drift = empirical `gap − period`, amount = latest posted amount.
Card charges are not shifted to business days.

**Not handled yet:** bills billed by day of month, single-charge streams,
secondary income streams.

### Dependencies (core)

| Crate | Version | Why |
|---|---|---|
| `nalgebra` | 0.35, `default-features = false, features = ["libm"]` | Statically sized `SMatrix<f64,N,N>` (on the stack, dimensions checked at compile time) and Cholesky. Works on WASM, no system dependencies. Alternative: hand-written 3×3 arrays with no dependency, but more of our own linear algebra to get wrong. Its cost is compile time. |
| `rand` | 0.10, `default-features = false` | `Rng`/`SeedableRng` traits only. Leaving out the OS RNG keeps `getrandom` out of the build, so wasm32-unknown-unknown builds without extra JS shims. |
| `rand_chacha` | 0.10 | ChaCha8 has a fixed, documented output stream that is the same on every platform, and it supports multiple streams. `StdRng` does not guarantee a stable algorithm. |
| `rand_distr` | 0.6, default features | Normal, ChiSquared, LogNormal (the last only for synth). Its default build uses `libm`, which is pure Rust, so the transcendental functions are bit-identical on native and WASM. **Do not enable `std_math`**, which changes results by platform. The same goes for anything that enables `num-traits/std`: nalgebra's `std` feature does, which is why nalgebra runs in `libm` mode (static matrices need no allocation). Check with `cargo tree -e features -i num-traits`. |
| `approx` | dev-only | `assert_relative_eq!`. nalgebra already depends on it, so it adds nothing. |
| `serde` | 1, **optional** (`serde` feature, off by default) | Derives for the versioned JSON contracts in `schema` (feed in, forecast report out; see docs/forecast-schema.md). Off by default so WASM/UniFFI builds that don't need JSON never compile it. It doesn't touch `num-traits`, so determinism is unaffected. |
| `ts-rs` | 12, **optional** (`ts` feature, off by default) | TypeScript declarations for the JSON contracts, rendered by a drift test in `ephemeris-wasm` against `web/src/schema.gen.ts` (docs/phase2-design.md §3). Only that crate's tests enable the feature; the CLI and WASM builds never compile it (`cargo tree -e normal` shows it absent). |

No `chrono`, `rayon`, or `proptest` in the core for Phase 1.

### Dependencies (CLI)

| Crate | Why |
|---|---|
| `clap` 4, `derive` | Subcommands, typed flags, range checks and `--help` for free. Hand-rolled parsing would be more code and worse errors. |
| `serde_json` 1, `float_roundtrip` | Reads and writes the JSON contracts (docs/forecast-schema.md). `float_roundtrip` makes every f64 parse back bit-identical, so a feed written and reread drives the tracker exactly as the in-memory one would. |
| `serde` 1 | Already in the graph through the core's `serde` feature. Named directly only for the generic read/write helpers. |

The Plaid adapter will add an HTTP client and `.env` loading; those will be
justified then. Versions get pinned through
the committed `Cargo.lock`. Cross-platform bit-identity also depends on the
crate versions, so upgrades must be deliberate.

## 5. Validation without Plaid

### Generator (`synth`)

`generate(cfg, seed) -> SynthRun { txns: Vec<Transaction>, truth: Vec<TruthDay{b,c,m}>,
today }` produces the **same `Transaction` type** the Plaid adapter does, so
everything after ingestion runs identically on both. It has two modes:

1. **Model-matched.** Data is generated exactly from §1 and §2 (Gaussian spend
   with mean `c` and variance `σ_s²`, random-walk `c` and `m`, income
   `N(m, R_inc)` on a fixed schedule, pending distortions with the same
   `σ_j`). In this mode the KF is the optimal estimator, so the
   statistical tests are **strict pass/fail**.
2. **Realistic.** The data deliberately departs from the model:
   - Poisson transaction counts with lognormal amounts, so spend is skewed and
     often zero
   - weekly seasonality (`weekday_factors`), which the filter deliberately
     doesn't model
   - steps in `c` (a move or a new habit)
   - paydays that land early or late, and bonuses
   - posting lag that skips weekends, tip and hold distortions, missing
     `authorized_date`
   - data gaps

   These tests are **reported, with loose regression bounds**. This is where we
   learn how far the model can be trusted.

Both modes also produce:
- own-account transfers between checking and savings, with the incoming leg
  invisible until it posts
- paydays from any `PaySchedule` with jitter plus weekend/holiday rollback
- subscriptions whose billing dates drift

Implemented so far: everything above except bonuses and data gaps, plus
step changes in `c` and `m` (`spend_steps`, `income_steps`, §6). Of the
scenarios below, only `regime_change` exists (`SynthConfig::regime_change`).

Named scenarios: `salaried`, `salaried_with_bonus`, `regime_change`, `gig`
(irregular income, expected to strain the per-event `m` model).

### Tests

- **Hand-computed unit tests:**
  - 1-state predict/update
  - 2-state constant-velocity step (the `P⁻` formula in §1)
  - g-day predict-only checked against the closed form
  - Joseph form against the standard form
  - the R aggregation for pending transactions
  - the adaptive estimator on a fixed innovation sequence
  - moments of the scaled-inverse-χ² sampler
  - MC with zero noise reproduces the deterministic path exactly
- **Invariants:** P stays symmetric PSD after every step across many seeded
  runs. Same seed gives bit-identical results, and a different seed gives
  different results. Path *i* is unchanged when N changes.
- **Filter consistency, model-matched, using ground truth:**
  - Run M = 50 seeds × 365 days.
  - Compute NEES `(x − x̂)ᵀ P⁻¹ (x − x̂)`.
  - After warm-up, ANEES must fall inside the 95% χ² band, ≈ [2.36, 3.72] for
    3 states. Also check NEES for each component on its own.
- **NIS** needs no ground truth, so it is **the same check we can later run on
  Plaid data**. It links synthetic validation to real data.
- **Accuracy:** RMSE of `b`, `c`, `m` against truth, compared with a 30-day
  trailing-mean burn rate and a naive linear projection. These are the
  baselines for the Phase 6 paper.
- **Adaptive convergence:** model-matched `R̂_inc` and `σ̂_s²` must end within
  a tolerance of the true values.
- **Forecast calibration:**
  - Filter up to day T, forecast to T + 90, and record whether the true `b`
    falls inside the 50/80/90/95% central intervals.
  - Across many seeds, coverage must sit inside a binomial tolerance
    (model-matched). We also record PIT histograms.
  - This is a small-scale Phase 6, built into the test suite.

Fast tests run under `cargo test`. The heavy calibration sweeps are
`#[ignore]`, run with `cargo test --release -- --ignored`, and the CLI exposes
them as `ephemeris validate --seed S --runs M`, which prints the metrics table.

## 6. Regime changes

Status: implemented 2026-10-21. The proposal below was written before any
code; "As built" at the end of this section records what measurement
changed.

**The problem.** `c` and `m` are random walks with small daily variances
(`q_c = 0.25`, so `c` drifts about $0.50/day per day). A *step*, such as a
move, a new habit, a raise or a job loss, takes the filter a long time to
follow. Its time constant on `c` is about `√(σ_s²/q_c)` = 80 days at
`σ_s = 40`. The Plaid sandbox showed the failure: spending started abruptly,
the spend rate stayed at $18.76/day against an actual $111.75/day, and
nothing in the estimate or the cone admitted it. `P_cc` stays at its steady
state, which is small, so the cone stays narrow around the wrong slope.

For income it is worse. A paycheck 20% smaller than `m` is gated as an
outlier (NIS > 10.83), so `m` is never updated, and every later paycheck is
gated too.

### Proposal: CUSUM on normalized innovations, then a covariance jump

**Detect** a shift in the *mean* of the innovations, not a burst of large
ones. When the model holds, the normalized innovations `e_k = ν_k/√S_k` are
white with unit variance. A step in `c` makes them biased: every day the
balance falls by about `Δc` more than predicted. A two-sided CUSUM (Page)
is the standard sequential test for such a bias:

```
e   = clip(ν/√S, ±√gate)                 the same Huber clip as the gate
g⁺ ← max(0, g⁺ + e − κ)                  g⁻ ← max(0, g⁻ − e − κ)
alarm when g⁺ > h or g⁻ > h
```

There is one detector per channel: balance (for `c`) and income (for `m`).
They run on settled days only, like the adaptive variances (§3).

**Respond** by treating the alarm as evidence that the state jumped. Each
side of the CUSUM tracks the mean innovation over its current run: `Δ̂ =
Σν / Σg` (`g` = days covered by each balance innovation; 1 per paycheck).
The jump variance `Δ̂²` is added to the predicted covariance as if the jump
happened at the start of the day:

```
P⁻ ← P⁻ + A·(Δ̂² e_i e_iᵀ)·Aᵀ             i = c for balance, m for income
```

For `c` this adds `Δ̂²` to `P_bb` and `P_cc` and `−Δ̂²` to `P_bc`. The
day's innovation is then recomputed and re-gated, the normal update runs,
and the detector resets. This is a one-step inflation of the process noise
(a jump in a jump-diffusion), not a permanent change in `q_c`:
- The next few observations get a large gain on `c` (or `m`), so the state
  catches up within days instead of months.
- `P_cc` shrinks again as the data comes in. The forecast draws
  `x₀ ~ N(x̂, P)`, so the cone is wide right after an alarm and narrows as
  the filter recovers. That is the behaviour we want: wide while unsure.
- Using the *clipped* `ν` makes `Δ̂` robust to a single lump. For a large
  step it underestimates, but the next alarm covers the rest.

### The tradeoff: false alarms against recovery speed

`κ` (the allowance, in σ) and `h` (the threshold) set both sides of the
tradeoff. Siegmund's approximation for Gaussian innovations gives:

| κ | h | false alarms per year (no change) | delay at 1σ | at 1.5σ | at 2σ |
|---|---|---|---|---|---|
| 0.25 | 5 | 5.1 | 7.3 d | 4.6 d | 3.4 d |
| 0.5 | 4 | 2.2 | 8.3 d | 4.7 d | 3.2 d |
| **0.5** | **5** | **0.8** | **10.3 d** | **5.7 d** | **3.9 d** |
| 0.5 | 6 | 0.3 | 12.3 d | 6.7 d | 4.6 d |
| 0.75 | 5 | 0.1 | 17.0 d | 7.3 d | 4.6 d |

The shift is in σ units of the daily spend noise: doubling `c = 60` at
`σ_s = 40` is a 1.5σ shift, and halving it is 0.75σ.

- **A false alarm costs** a temporarily wider cone and a noisier `c`. `P_cc`
  goes from its steady state (~20, sd ≈ $4.5/day) to roughly the run's
  `Δ̂²` (a few hundred) and relaxes over a few weeks. The estimate itself
  barely moves, because the innovations were unbiased.
- **A missed or late alarm costs** a confident cone around the wrong slope.
  That is the failure we are fixing, and it is worse.
- Below about 0.5σ the CUSUM is slow (weeks), but the random walk on `c`
  already follows small drifts, so the two complement each other.

Proposed default: **κ = 0.5, h = 5** on both channels. It needs measuring
on the realistic generator, whose heavy tails and weekly pattern push NIS
above χ²₁ and will raise the false-alarm rate. On the income channel, a
single bonus raises `g⁺` by at most `√gate − κ ≈ 2.8 < h`, so one bonus never
alarms; two consecutive paychecks off by more than about 2σ do.

### Alternatives considered

- **A run of large NIS values** (windowed `ΣNIS` against χ²_W). It detects
  variance as well as bias, so it confuses a level shift with heavy-tailed
  spending, lumps and the weekly pattern, all of which inflate NIS without
  any change in `c`. It also needs a window, which fixes the detection delay
  whatever the shift size. The signed CUSUM uses the direction, which is what
  a step changes.
- **Fading memory** (inflating `P` every day by a factor). It is always on,
  so it pays the noise cost every day to buy speed it rarely needs.
- **Sage–Husa / covariance matching for `q_c`.** It runs on the §3
  machinery, but `q_c` is weakly identified from balance data and a step is
  not a random walk: it would learn a large `q_c` for ever after one move.
- **Interacting multiple models (IMM)** with a "jump" mode. This is the
  principled upgrade. It costs a bank of filters and mode probabilities, and
  it loses the hand-checkable single update. It is worth revisiting if the
  CUSUM's false alarms prove costly on real data.

### Tests

- The synthetic generator gets step changes: on a given day, spending
  (`c`) or income (`m`) is multiplied by a configurable factor.
- Recovery time: days from the step until `|ĉ − c|` stays within 25% of the
  step for 14 days, with and without the detector.
- Forecast coverage in the weeks after the change. Origins 1–4 weeks after
  the step, 30-day horizon, 90% interval coverage, with and without the
  detector. The cone must be wider than in the same run with no step.
- False-alarm rate with no step, on model-matched and realistic data.

### As built

Code: `change.rs` (the CUSUM), `FinanceFilter::add_jump`,
`VarianceEstimator::forget`, and the tracker's `spend_change`,
`max_spend_jump` and `income_change` settings. Tests:
`tests/regime_change.rs`, plus unit tests in each module. Measuring changed
four things against the proposal:

1. **The jump stays zero-mean.** A variant that also moved the mean by `Δ̂`
   (the run's maximum-likelihood shift) did no better after real steps. It
   did worse after false alarms, which then moved `c` by the full `Δ̂`:
   realistic no-step `c` RMSE 12.7 against 10.7 $/day.
2. **The spend jump is capped at `σ̂_s²`** (`max_spend_jump = 1`). On lumpy
   spending a false alarm can have `Δ̂ ≈ 3.3σ̂`, and after a jump that size
   the next single day sets `ĉ` almost by itself. With the cap, one day
   moves `c` by at most about half its deviation. A large real change sets
   off several alarms in a row instead (a unit test checks this), and
   recovery got faster: realistic ×2 from 55 to 45 days at h = 5. Income is
   not capped. Its `Δ̂` is already bounded by the gate, and a cap at `R̂_inc`
   would be far too small for a pay cut.
3. **h = 6 on the balance channel, h = 5 on income.** With heavy-tailed
   spending, h = 5 gave 1.5–1.7 false alarms a year and doubled the
   realistic no-step `c` RMSE. Income stays at h = 5 so that two short
   paychecks in a row are enough. At h = 6 two clipped innovations
   (2 × 2.79) would not alarm, and a pay cut would wait a third pay period.
4. **On an alarm the noise estimate forgets** back to its prior count, so
   `σ̂_s²` (or `R̂_inc`) can follow a change in spread too: doubling realistic
   spending also doubles its sd. **The detector also runs on the provisional
   tail.** Its state there is thrown away with the tail, so a change shows
   in the live estimate 2–5 days before it settles. Tail-only alarms appear
   on 0.3–0.4% of days with no change. That roughly doubled the 1-week
   coverage after a doubling, from 20% to 52% (h = 5, uncapped).

**Step changes** (50 seeds each; the step on day 182; median days to the
first alarm and to recovery, defined as `|ĉ − c|` within 25% of the step for
14 days; 30-day forecasts from 1–4 weeks after the step):

| Scenario | Detector | Alarm | Recovery | Recovered ≤ 30 d | 90% coverage at +7 / +14 / +21 / +28 d |
|---|---|---|---|---|---|
| matched, spend ×2 | on | 9 d | 16 d | 92% | 44 / 82 / 90 / 92% |
| | off | – | never | 0% | 0 / 0 / 0 / 12% |
| matched, spend ×0.5 | on | 31 d | 67 d | 20% | 22 / 48 / 56 / 72% |
| | off | – | never | 0% | 20 / 22 / 26 / 66% |
| realistic, spend ×2 | on | 17 d | 41 d | 30% | 10 / 42 / 50 / 72% |
| | off | – | never | 0% | 6 / 12 / 14 / 22% |
| realistic, spend ×0.5 | on | – | 106 d | 2% | 8 / 20 / 30 / 80% |
| | off | – | never | 2% | 8 / 20 / 30 / 76% |
| matched, paycheck ×0.8 | on | 22 d | 18 d | 70% | 10 / 10 / 94 / 90% |
| | off | – | never | 0% | 6 / 6 / 6 / 28% |
| matched, paycheck ×1.1 | on | 36 d | 45 d | 26% | 66 / 72 / 84 / 88% |
| | off | – | never | 0% | 66 / 72 / 74 / 88% |

"Never" means not within the 120 days after the step. Two to four weeks
after a doubling, the 90% cone widens to 1.4–1.9× its no-step width, and
coverage goes from almost never to about nominal on model-matched data.

**Limits, from the same table.**

- **The first week is still confident.** Detection takes the CUSUM delay
  plus, for settled alarms, the settlement lag.
- **Halving spending is a 0.75σ shift and is found slowly** (a month),
  where it is found at all. On realistic data its skew hides it: few days
  spend much more than the mean, so the clipped innovations barely move.
- **A pay cut is seen after the second short paycheck**, by design. Until
  then it is indistinguishable from a one-off deduction, so forecasts in
  the first two weeks miss it.
- **Realistic recovery is slow even after detection.** A doubling there
  also doubles the daily spread, to about $122, and pinning `c` to within
  $17.50/day takes about 7 weeks of data whatever the detector does.

**False alarms and their cost** (no step; same sweeps as §5):

| | Model-matched | Realistic |
|---|---|---|
| Settled false alarms per year (balance channel) | 0.30 | 0.48 |
| RMSE of `c`, $/day: detector off → on | 4.67 → 5.27 | 6.44 → 7.55 |
| 2σ coverage of `c` | 96.1% → 96.2% | 91.6% → 91.5% |
| MC CRPS at 7 / 30 / 90 days, off | 217 / 210 / 421 | 192 / 241 / 519 |
| MC CRPS at 7 / 30 / 90 days, on | 217 / 211 / 424 | 194 / 251 / 560 |

The cost shows mostly at 90 days on realistic data: each false alarm
leaves `ĉ` noisier for a few weeks, and 90 days multiplies that.
Model-matched coverage with the detector on drifts up by about one point
(the 50% interval at 90 days: 54.0% → 55.3%). That pushed one cell of the
asserted calibration check one forecast past its ±5-point bound, so the
strict ±5-point check (`tests/forecast_calibration.rs`) now runs with
detection off, the model's own assumptions. The default tracker is asserted
separately within ±8 points.

## 7. The negative spend-rate band: a spend floor

Status: built 2026-10-29 behind `ForecastConfig::spend_floor`, **off by
default** until the measurements below are reviewed. Turning it on changes
every path, so the golden files are regenerated in the same change.
The proposal is kept as written; see "Measured" for what held up.

### The problem

`c` is one coordinate of a linear-Gaussian state, so its marginal is
`N(ĉ, P_cc)` and nothing stops it going below zero. After its three regime
changes, the Plaid sandbox has `ĉ = $67.04/day` with `σ_c = $36.04`. The
state panel then prints "±2σ −$5.04 to $139.11", and 3.1% of the Monte
Carlo paths (`Φ(−1.86)`) start out *earning* money every day. That causes
two problems:

- **Display.** A negative lower bound reads as "you might be earning",
  when it really means "very unsure".
- **Forecast.** Those paths climb steadily, so they sit at the top of the
  cone and pull up its upper quantiles. They also slightly lower the
  chance of dipping below $0.

### Options

**A. Log space: track `ℓ = log c` instead of `c`.** Rejected.
- `b_k = b_{k−1} − exp(ℓ) + …` is nonlinear, so the tracker becomes an
  EKF/UKF. That gives up the exact linear filter and the NEES/ANEES
  consistency gates in §5, which assume it.
- It makes `c ≤ 0` impossible, not just unlikely. But net discretionary
  flow can be an inflow (refunds, reimbursements), which is why the README
  calls this "not impossible".
- It changes the model everywhere, not only near zero. The model-matched
  generator has additive Gaussian `c`, so a log model would be
  misspecified against it. The ±5-point calibration check would then
  measure the misspecification rather than the filter, and the generator
  would have to change too.
- The noise adaptation (§3) and the regime detector (§6) work on additive
  innovations in `c`, and both would need rework.

**B. Truncate at a floor, at the outputs only.** Recommended.
- The tracker stays exactly as it is: linear, unconstrained, with the same
  NEES/NIS behavior. The constraint is prior knowledge applied where the
  estimate is used.
- **Floor.** Add `ForecastModel::c_floor: Option<f64>`, default `Some(0.0)`.
  `None` keeps today's behavior for a user whose discretionary flow really
  is a net inflow.
- **Monte Carlo start.** Draw `c₀` from `N(ĉ, P_cc)` truncated to
  `c ≥ floor`, by inverse CDF. Then draw `(b₀, m₀)` from their Gaussian
  conditional on that `c₀`, which keeps the b–c correlation exact. Drawing
  the full vector and rejecting would change the RNG draw count per path.
- **Monte Carlo steps.** Reflect the random-walk step at the floor
  (`c ← 2·floor − c` when `c < floor`). Reflection keeps the step variance
  and needs no extra draw.
- **Report.** Add `spend_rate.low`/`high`: the truncated distribution's
  2.275% and 97.725% quantiles, the same coverage as ±2σ. Compute them in
  the core with the existing `libm`, so no new crate is needed. The app
  labels them "95% range" instead of ±2σ. `mean` and `sd` stay the
  filter's. The change is additive, so the schema version stays 1.

### Calibration impact

These are hand computations from the closed-form truncated normal. They
have not been measured yet.

- **Existing sweeps: no material change.** The synthetic truth starts at
  `c = $60/day` and walks with `q_c = 0.25`, so its spread after a year is
  about $9.50. The tracker's `c` RMSE is $4.67 (model-matched) and $6.44
  (realistic). Mass below zero is about 10⁻¹⁰ at most, so the floor
  essentially never binds. The ±5-point (detector off) and ±8-point
  (default) coverage checks should move only by Monte Carlo noise.
- **That noise exists because every path changes.** Splitting the `x₀`
  draw into "c first, then (b, m) given c" changes how the normal draws
  map to states, even when the floor is inactive. The golden files in
  `crates/ephemeris-wasm/tests/golden/` and the CLI's printed numbers will
  change and must be regenerated. The byte-identity proof (native = WASM)
  still holds.
- **Where the floor binds (sandbox numbers):**

  | | now | truncated |
  |---|---|---|
  | `c` band | −$5.04 to $139.11 | $8.94 to $139.60 |
  | mean `c` | $67.04/day | $69.67/day (+$2.63) |
  | 5th percentile of `c₀` | $7.76 | $16.37 |

  - **Median balance** drops by about $18 at day 7, $79 at day 30 and $237
    at day 90. *(Wrong: this applies the mean shift of `c` to the median.
    The median of truncated `c` moves by only $1.42/day, so about $128 at
    day 90. Measured: $115.)*
  - **Upper (95%) quantile at day 90** drops by up to about $770. That
    figure is `90 × (16.37 − 7.76)` and assumes `c` dominates the spread;
    daily noise and lumps make the real drop smaller.
  - **Lower tail** barely moves.
  - **P(below $0)** rises slightly; the headline currently shows 45%.
- **Effect on accuracy.** Low ĉ/σ_c happens right after a regime change
  and on short histories. That is the "first week after a regime change is
  overconfident" gap, but on the upper side. Truncation narrows the upper
  side only, and only by removing paths that can't happen if spending is
  really ≥ 0. So it should improve calibration there, not hurt it.

### How to measure it before merging

1. **Existing sweeps.** Run both sweeps (`scripts/test-full.sh
   --nocapture`) with the floor on and off. Coverage and CRPS should agree
   within Monte Carlo noise.
2. **A new sweep where the floor binds.** Model-matched, with
   `initial.c = $10/day`, a short warm-up so `σ_c` stays large, and the
   generator's truth also floored at 0 (otherwise the truth itself goes
   negative). Compare 90% coverage and CRPS at 7/30/90 days, floor on vs
   off. Expected: floor-off over-covers on the upper side; floor-on is
   within ±5 points.
3. **Sandbox.** Re-run it and record the new band and cone in
   `docs/sandbox-comparison.md`.

### As built

- **Sampling, not clamping.** Each path's starting `c` is drawn from its
  Gaussian conditioned on `c ≥ floor`, by inverse CDF (`truncated.rs`:
  `libm` erfc, AS241 quantile). One uniform per path, exact in any tail.
  A clamp would pile the excluded mass on `c = floor`, where those paths
  would spend nothing. Rejection sampling gives the same distribution,
  but with a variable number of draws and no bound on the loop.
- **Correlation kept.** `P` is factored in the order `(c, b, m)`, so `c₀`
  depends on the first normal draw alone. Truncating that draw leaves
  `(b, m)` exactly Gaussian given `c₀`.
- **Random walk.** The step reflects at the floor (`c ← 2·floor − c`),
  with no extra draw.
- **Report.** `spend_rate_range` (`docs/forecast-schema.md`). The CLI and
  the app show it in place of ±2σ when present.
- **Generator.** `SynthConfig::c_floor` reflects the true `c` the same way.

### Measured (2026-10-29; floor on vs off, same seeds)

**Existing sweeps (100 seeds, 300 forecasts per cell).** The floor never
binds here, so every change is Monte Carlo noise from the redrawn paths.
The asserted ±5 and ±8 point coverage checks pass with the floor on. The
largest changes are below; the full before/after tables are in the
review notes.

| sweep | h | CRPS off → on | 90% coverage off → on |
|---|---|---|---|
| realistic | 90 | 560.44 → 561.56 | 89.0 → 88.7 |
| model-matched, default tracker | 7 | 217.04 → 217.79 | 91.0 → 89.7 |
| model-matched, no detection | 90 | 420.89 → 421.80 | 92.0 → 91.7 |

**Where it binds** (`spend_floor_sweep`: low spenders, true `c` floored
at 0). On average 19% of the estimate's `c` Gaussian sits below $0 at
the forecast origins in model-matched runs, and 2.8% in realistic runs.

| case | h | CRPS off → on | RMSE off → on | 90% cov. off → on | 90% width off → on |
|---|---|---|---|---|---|
| matched, $5/day, no detection | 30 | 208.92 → 203.86 | 390.6 → 383.5 | 93.0 → 93.0 | 1345 → 1306 |
| matched, $5/day, no detection | 90 | 414.56 → 384.77 | 735.3 → 686.3 | 91.7 → 92.7 | 2705 → 2526 |
| matched, $5/day, default | 90 | 416.90 → 388.70 | 737.2 → 692.2 | 91.7 → 92.7 | 2749 → 2553 |
| realistic, $10/day, default | 90 | 317.40 → 313.36 | 554.5 → 550.7 | 94.0 → 92.0 | 2277 → 2237 |

On the 90-day horizon the floor cuts CRPS by 7% (model-matched) and the
90% band by 7%, while coverage stays within ±5 points of nominal. At 7
days nothing changes beyond noise.

**Regime sweep.** All changes are within noise (50 seeds, so coverage
moves in 2-point steps). The exception is the one place the floor should
act: after a detected halving of spending (the covariance jump widens
`c`), the 14-day 90% width drops from 1476 to 1330 at the same 48%
coverage.

**Plaid sandbox** (2000 paths, seed 0):

| | floor off | floor on |
|---|---|---|
| spend band | −$5.04 to $139.11 (±2σ) | $8.94 to $139.59 |
| share of `c` Gaussian below $0 | 3.14% | (removed) |
| median balance, day 7 / 30 / 90 | $749.25 / $1,594.91 / $4,892.77 | $735.74 / $1,585.24 / $4,777.77 |
| 95% balance, day 90 | $10,634.77 | $9,797.83 (−$837) |
| 5% balance, day 90 | −$913.70 | −$882.07 |
| P(below $0 at some point by day 90) | 44.75% | 46.70% |

## Decisions (resolved 2026-10-19)

1. **Accounts:** checking and savings only. Own-account transfers net to zero
   and never count as spending or income (§0, §2).
2. **Income:** one primary stream for Phase 1, so N = 3.
3. **Pay schedules:** weekly, biweekly, semi-monthly and monthly, with
   weekend and holiday shifts to the previous business day. No "last business
   day" (§1).
4. **Spending:** "recurring-expense baseline" is confirmed as scheduled flows
   (`u`) plus a smooth discretionary rate (`c`).
5. **Day-of-week effects:** deferred in the model, kept in the realistic
   generator so we can measure what ignoring them costs.
6. **Paychecks:** a paycheck enters `b` through `B·u` at its actual amount and
   updates `m` as a separate observation, so it never appears as a balance
   innovation (§1).
