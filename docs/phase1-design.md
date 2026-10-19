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
  `FlowKind::Transfer`. Transfers are never spending or income:
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
an external (non-transfer) flow. The generator produces exactly this
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
  observe.rs    ObservedFeed::new(snapshot, ledger_balance, today): roles (incl. transfer
                matching), settled_through, opening balance, FeedDay { u, balance: Option<(z,R)>,
                income: Vec<f64> } for settled days and the tail, shared_variance
  tracker.rs    Tracker::ingest(snapshot, ledger_balance, today): checkpoint + provisional
                tail, gating, adaptive noise; rebuilds from the prior if consumed settled
                days change; returns DayReport per day
  schedule.rs   recurring streams -> period, phase, jitter, amount (feeds the MC)
  forecast.rs   seeded Monte Carlo -> Cone
  synth.rs      seeded generator with ground truth (§5)
  metrics.rs    NEES, NIS, RMSE, interval coverage (shared by tests and CLI)
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

### Dependencies (core)

| Crate | Version | Why |
|---|---|---|
| `nalgebra` | 0.35, `default-features = false, features = ["libm"]` | Statically sized `SMatrix<f64,N,N>` (on the stack, dimensions checked at compile time) and Cholesky. Works on WASM, no system dependencies. Alternative: hand-written 3×3 arrays with no dependency, but more of our own linear algebra to get wrong. Its cost is compile time. |
| `rand` | 0.10, `default-features = false` | `Rng`/`SeedableRng` traits only. Leaving out the OS RNG keeps `getrandom` out of the build, so wasm32-unknown-unknown builds without extra JS shims. |
| `rand_chacha` | 0.10 | ChaCha8 has a fixed, documented output stream that is the same on every platform, and it supports multiple streams. `StdRng` does not guarantee a stable algorithm. |
| `rand_distr` | 0.6, default features | Normal, ChiSquared, LogNormal (the last only for synth). Its default build uses `libm`, which is pure Rust, so the transcendental functions are bit-identical on native and WASM. **Do not enable `std_math`**, which changes results by platform. The same goes for anything that enables `num-traits/std`: nalgebra's `std` feature does, which is why nalgebra runs in `libm` mode (static matrices need no allocation). Check with `cargo tree -e features -i num-traits`. |
| `approx` | dev-only | `assert_relative_eq!`. nalgebra already depends on it, so it adds nothing. |

No `serde`, `chrono`, `rayon`, or `proptest` in the core for Phase 1. The CLI
will need `serde`/`serde_json`, an HTTP client, and `.env` loading for Plaid;
those will be justified when the CLI is designed. Versions get pinned through
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

Implemented so far: everything above except steps in `c`, bonuses, data gaps
and the scenarios below.

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
