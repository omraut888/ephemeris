//! The multi-replica convergence simulation (docs/phase4b-design.md §5).
//!
//! A simulated bank changes underneath several replicas. Two of them fetch
//! from it (complete fetches first, then deltas with removals). All of
//! them make user edits. Each replica's clock is skewed by up to a day and
//! jumps back now and then. Replicas publish sealed files, and files are
//! delivered in random order, some stale, some twice, as a synced folder
//! would deliver them. At the end everyone gossips until nothing changes,
//! and the test asserts:
//!
//! 1. **Identical:** every replica holds byte-identical state, materializes
//!    a byte-identical feed, and computes a byte-identical forecast.
//! 2. **Correct:** after a final fetch, the live records equal the bank's
//!    final records, and the feed equals the feed built straight from them.
//! 3. **Intent:** every register holds the edit with the largest stamp
//!    made to it, and whenever one edit causally followed another on the
//!    same key (by the simulator's own happens-before tracking, never by
//!    stamps), it has the larger stamp, whatever the clock skews.
//!
//! A failure names its seed; `SEED=<n> cargo test -p ephemeris-sync --test
//! convergence one_seed -- --ignored` reruns just that one.

use std::collections::{BTreeMap, BTreeSet};

use ephemeris_core::FlowKind;
use ephemeris_core::json::forecast_json;
use ephemeris_core::schema::Date;
use ephemeris_sync::edits::{Edit, rule_key};
use ephemeris_sync::envelope::{Key, open, seal};
use ephemeris_sync::ingest::FetchBatch;
use ephemeris_sync::state::{SavedQuery, Setting};
use ephemeris_sync::{AccountSnapshot, BankRecord, Category, Hlc, ReplicaId, SyncState, tag};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

const KEY: Key = [0x42; 32];
const MIN: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
const SKEWS: [i64; 5] = [-DAY_MS, -5 * MIN, 0, 5 * MIN, DAY_MS];
const MERCHANTS: [&str; 5] = ["Cafe Uno", "Grocer", "Fuel Stop", "Book Barn", "Pizza Pie"];
const START: i32 = 20_000; // 2024-10-04

// ---------------------------------------------------------------- the bank

struct Bank {
    today: i32,
    records: BTreeMap<String, BankRecord>,
    /// Every id the bank touched, in order: a fetch cursor is an index.
    log: Vec<String>,
    next: u64,
}

fn record(id: &str, account: &str, day: i32, amount: f64, name: &str) -> BankRecord {
    BankRecord {
        id: id.into(),
        account: account.into(),
        date: Date(day),
        authorized_date: None,
        pending: false,
        pending_id: None,
        amount,
        currency: Some("USD".into()),
        merchant_name: Some(name.into()),
        name: Some(name.to_uppercase()),
        category: None,
    }
}

impl Bank {
    /// 90 days of posted history: paychecks, a bill, everyday spending.
    fn new(rng: &mut ChaCha8Rng) -> Bank {
        let mut b = Bank {
            today: START,
            records: BTreeMap::new(),
            log: Vec::new(),
            next: 0,
        };
        for _ in 0..90 {
            b.new_day(rng);
            for _ in 0..rng.random_range(0..3) {
                let r = b.purchase(rng, false);
                b.add(r);
            }
        }
        b
    }

    fn id(&mut self) -> String {
        self.next += 1;
        format!("t{:05}", self.next)
    }

    fn add(&mut self, r: BankRecord) {
        self.log.push(r.id.clone());
        self.records.insert(r.id.clone(), r);
    }

    fn remove(&mut self, id: &str) {
        self.log.push(id.to_string());
        self.records.remove(id);
    }

    fn purchase(&mut self, rng: &mut ChaCha8Rng, pending: bool) -> BankRecord {
        let id = self.id();
        // Sub-cent amounts now and then, as Plaid's sandbox has.
        let cents = -(rng.random_range(150..9000) as f64) / 100.0;
        let amount = if rng.random_bool(0.2) {
            cents - rng.random_range(1..99) as f64 / 10_000.0
        } else {
            cents
        };
        let name = MERCHANTS[rng.random_range(0..MERCHANTS.len())];
        BankRecord {
            pending,
            ..record(&id, "chk", self.today, amount, name)
        }
    }

    fn pendings(&self) -> Vec<String> {
        self.records
            .values()
            .filter(|r| r.pending)
            .map(|r| r.id.clone())
            .collect()
    }

    fn new_day(&mut self, rng: &mut ChaCha8Rng) {
        self.today += 1;
        let n = self.today - START;
        if n % 14 == 0 {
            let id = self.id();
            let pay = 2400.0 + rng.random_range(-500..500) as f64 / 100.0;
            self.add(record(&id, "chk", self.today, pay, "Acme Payroll"));
        }
        if n % 30 == 3 {
            let id = self.id();
            let mut r = record(&id, "chk", self.today, -61.5, "City Water");
            r.category = Some(Category {
                primary: "RENT_AND_UTILITIES".into(),
                detailed: "RENT_AND_UTILITIES_WATER".into(),
            });
            self.add(r);
        }
    }

    /// One random change, as a bank makes them.
    fn step(&mut self, rng: &mut ChaCha8Rng) {
        let pendings = self.pendings();
        match rng.random_range(0..10) {
            0..=3 => {
                let r = self.purchase(rng, true);
                self.add(r);
            }
            4 if !pendings.is_empty() => {
                // A tip or a hold settles: the pending amount changes.
                let id = &pendings[rng.random_range(0..pendings.len())];
                let mut r = self.records[id].clone();
                r.amount -= rng.random_range(1..800) as f64 / 100.0;
                self.add(r);
            }
            5 if !pendings.is_empty() => {
                let id = pendings[rng.random_range(0..pendings.len())].clone();
                self.remove(&id);
            }
            6..=7 if !pendings.is_empty() => {
                // It posts under a new id that names the pending one.
                let p = self.records[&pendings[rng.random_range(0..pendings.len())]].clone();
                let id = self.id();
                let mut q = BankRecord {
                    id,
                    pending: false,
                    pending_id: Some(p.id.clone()),
                    date: Date(self.today),
                    authorized_date: Some(p.date),
                    ..p.clone()
                };
                if rng.random_bool(0.3) {
                    q.amount -= 1.0;
                }
                self.remove(&p.id);
                self.add(q);
            }
            8 => {
                // An own-account transfer: both legs post the same day.
                let amount = rng.random_range(50..500) as f64;
                let (o, i) = (self.id(), self.id());
                let mut out = record(&o, "chk", self.today, -amount, "To Savings");
                let mut inc = record(&i, "sav", self.today, amount, "From Checking");
                out.category = Some(Category {
                    primary: "TRANSFER_OUT".into(),
                    detailed: "TRANSFER_OUT_ACCOUNT_TRANSFER".into(),
                });
                inc.category = Some(Category {
                    primary: "TRANSFER_IN".into(),
                    detailed: "TRANSFER_IN_ACCOUNT_TRANSFER".into(),
                });
                self.add(out);
                self.add(inc);
            }
            _ => self.new_day(rng),
        }
    }

    fn accounts(&self) -> Vec<AccountSnapshot> {
        ["chk", "sav"]
            .iter()
            .map(|a| AccountSnapshot {
                id: (*a).into(),
                depository: true,
                current: Some(
                    1000.0
                        + self
                            .records
                            .values()
                            .filter(|r| r.account == *a && !r.pending)
                            .map(|r| r.amount)
                            .sum::<f64>(),
                ),
                currency: Some("USD".into()),
            })
            .collect()
    }

    /// A complete fetch (`since` = None) or the changes since a cursor.
    fn fetch(&self, since: Option<(usize, i32)>) -> FetchBatch {
        match since {
            None => FetchBatch {
                today: Date(self.today),
                window_start: Date(START),
                records: self.records.values().cloned().collect(),
                removed: Vec::new(),
                accounts: self.accounts(),
                complete: true,
            },
            Some((cursor, last_today)) => {
                let touched: BTreeSet<&String> = self.log[cursor..].iter().collect();
                FetchBatch {
                    today: Date(self.today),
                    window_start: Date(last_today),
                    records: touched
                        .iter()
                        .filter_map(|id| self.records.get(*id).cloned())
                        .collect(),
                    removed: touched
                        .iter()
                        .filter(|id| !self.records.contains_key(**id))
                        .map(|id| (*id).clone())
                        .collect(),
                    accounts: self.accounts(),
                    complete: false,
                }
            }
        }
    }
}

// ------------------------------------------------------------ the replicas

struct Replica {
    id: ReplicaId,
    state: SyncState,
    skew: i64,
    fetcher: bool,
    /// Cursor and `today` of this fetcher's last fetch.
    cursor: Option<(usize, i32)>,
    /// Edits this replica has seen (made, or merged from a file): the
    /// simulator's own happens-before, independent of stamps.
    seen: BTreeSet<usize>,
}

/// One published file: the sealed bytes and the edits its writer had seen.
type File = (Vec<u8>, BTreeSet<usize>);

struct EditRecord {
    key: String,
    stamp: Hlc,
    /// Edits that happened before this one.
    deps: BTreeSet<usize>,
    /// The register right after the edit, as JSON.
    register: String,
}

/// The register an edit wrote, as JSON, and its key.
fn register(state: &SyncState, key: &str) -> String {
    let (kind, k) = key.split_at(2);
    match kind {
        "o:" => serde_json::to_string(&state.overrides[k]),
        "r:" => serde_json::to_string(&state.rules[k]),
        "s:" => {
            serde_json::to_string(&state.settings[&serde_json::from_str::<Setting>(k).unwrap()])
        }
        "f:" => serde_json::to_string(&state.fetcher),
        _ => serde_json::to_string(&state.queries[k]),
    }
    .unwrap()
}

fn random_kind(rng: &mut ChaCha8Rng) -> Option<FlowKind> {
    [
        None,
        Some(FlowKind::Discretionary),
        Some(FlowKind::Scheduled),
        Some(FlowKind::Transfer),
        Some(FlowKind::Income),
    ][rng.random_range(0..5)]
}

fn random_query(rng: &mut ChaCha8Rng) -> SavedQuery {
    SavedQuery {
        amount_cents: rng.random_range(0..500_000),
        by: Date(START + rng.random_range(90..200)),
        label: ["trip", "rent", ""][rng.random_range(0..3)].into(),
    }
}

/// A random edit this replica could make now, and the register key it writes.
fn random_edit(rng: &mut ChaCha8Rng, r: &Replica) -> Option<(Edit, Option<String>)> {
    let s = &r.state;
    Some(match rng.random_range(0..6) {
        0 | 1 => {
            let ids: Vec<&String> = s.records.keys().collect();
            if ids.is_empty() {
                return None;
            }
            let id = ids[rng.random_range(0..ids.len())].clone();
            (
                Edit::Override {
                    id: id.clone(),
                    kind: random_kind(rng),
                },
                Some(format!("o:{id}")),
            )
        }
        2 => {
            let name = MERCHANTS[rng.random_range(0..MERCHANTS.len())];
            (
                Edit::Rule {
                    counterparty: name.into(),
                    kind: random_kind(rng),
                },
                Some(format!("r:{}", rule_key(name))),
            )
        }
        3 => {
            let (setting, value) = match rng.random_range(0..3) {
                0 => (Setting::HorizonDays, rng.random_range(10..60)),
                1 => (Setting::Paths, rng.random_range(10..60)),
                _ => (Setting::Seed, rng.random()),
            };
            let key = format!("s:{}", serde_json::to_string(&setting).unwrap());
            (Edit::Set { setting, value }, Some(key))
        }
        4 => (
            Edit::AddQuery {
                query: random_query(rng),
            },
            None, // the key is the id the edit returns
        ),
        _ => {
            let ids: Vec<String> = s.saved_queries().map(|(id, _)| id.to_string()).collect();
            if ids.is_empty() {
                return None;
            }
            let id = ids[rng.random_range(0..ids.len())].clone();
            let edit = if rng.random_bool(0.5) {
                Edit::DeleteQuery { id: id.clone() }
            } else {
                Edit::EditQuery {
                    id: id.clone(),
                    query: random_query(rng),
                }
            };
            (edit, Some(format!("q:{id}")))
        }
    })
}

struct World {
    rng: ChaCha8Rng,
    bank: Bank,
    replicas: Vec<Replica>,
    /// Every version each replica has published, oldest first.
    files: Vec<Vec<File>>,
    edits: Vec<EditRecord>,
    /// True time in ms.
    time: i64,
    /// The step at which replica 1 takes the fetcher role from replica 0.
    handover_at: Option<usize>,
    /// Fetches the one-fetcher rule refused.
    refused: usize,
}

impl World {
    fn new(seed: u64, n: usize, fetchers: Fetchers) -> World {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let bank = Bank::new(&mut rng);
        let replicas = (0..n)
            .map(|i| Replica {
                id: rng.random::<u64>() | 1, // never 0, distinct in practice
                state: SyncState::new(),
                skew: SKEWS[rng.random_range(0..SKEWS.len())],
                fetcher: i == 0,
                cursor: None,
                seen: BTreeSet::new(),
            })
            .collect();
        let handover_at = match fetchers {
            Fetchers::One => None,
            Fetchers::Handover => Some(rng.random_range(50..350)),
        };
        World {
            rng,
            bank,
            replicas,
            files: vec![Vec::new(); n],
            edits: Vec::new(),
            time: 1_790_000_000_000,
            handover_at,
            refused: 0,
        }
    }

    /// The fetcher the (converged) state names.
    fn fetcher(&self) -> usize {
        let id = self.replicas[0].state.fetcher.as_ref().unwrap().value;
        self.replicas.iter().position(|r| r.id == id).unwrap()
    }

    fn now(&self, i: usize) -> u64 {
        (self.time + self.replicas[i].skew).max(0) as u64
    }

    /// A delta fetch from this fetcher's cursor (complete the first time),
    /// or a complete fetch when `complete`.
    fn fetch(&mut self, i: usize, complete: bool) -> bool {
        let since = if complete {
            None
        } else {
            self.replicas[i].cursor
        };
        let batch = self.bank.fetch(since);
        let now = self.now(i);
        let r = &mut self.replicas[i];
        let claims = r.state.fetcher.is_none();
        match r.state.ingest(&batch, r.id, now) {
            Ok(_) => r.cursor = Some((self.bank.log.len(), self.bank.today)),
            Err(e) => {
                // Only the one-fetcher rule may refuse a fetch here.
                assert!(e.contains("fetched by device"), "{e}");
                self.refused += 1;
                return false;
            }
        }
        if claims {
            // The first fetch claims the fetcher role: a write like an edit.
            self.record_edit(i, "f:fetcher".into());
        }
        true
    }

    /// Note the write replica `i` just made to `key`, for the intent checks.
    fn record_edit(&mut self, i: usize, key: String) {
        let n = self.edits.len();
        let r = &mut self.replicas[i];
        self.edits.push(EditRecord {
            stamp: r.state.clocks[&r.id],
            register: register(&r.state, &key),
            key,
            deps: r.seen.clone(),
        });
        r.seen.insert(n);
    }

    /// Replica `i` takes the fetcher role over (design §1.4).
    fn take_fetcher(&mut self, i: usize) {
        let now = self.now(i);
        let r = &mut self.replicas[i];
        r.state.edit(&Edit::TakeFetcher, r.id, now).unwrap();
        r.fetcher = true;
        self.record_edit(i, "f:fetcher".into());
    }

    fn edit(&mut self, i: usize) {
        let Some((edit, key)) = random_edit(&mut self.rng, &self.replicas[i]) else {
            return;
        };
        let now = self.now(i);
        let r = &mut self.replicas[i];
        let new_id = r.state.edit(&edit, r.id, now).unwrap();
        let key = key.unwrap_or_else(|| format!("q:{}", new_id.unwrap()));
        let n = self.edits.len();
        self.edits.push(EditRecord {
            stamp: r.state.clocks[&r.id],
            register: register(&r.state, &key),
            key,
            deps: r.seen.clone(),
        });
        r.seen.insert(n);
    }

    fn publish(&mut self, i: usize) {
        let mut nonce = [0u8; 24];
        self.rng.fill(&mut nonce[..]);
        let r = &self.replicas[i];
        self.files[i].push((seal(&r.state, r.id, &KEY, &nonce), r.seen.clone()));
    }

    /// Deliver some version of `from`'s file to `to`: often the latest,
    /// otherwise any older one (a lagging provider).
    fn deliver(&mut self, from: usize, to: usize) {
        let versions = &self.files[from];
        if versions.is_empty() || from == to {
            return;
        }
        let k = if self.rng.random_bool(0.5) {
            versions.len() - 1
        } else {
            self.rng.random_range(0..versions.len())
        };
        let (bytes, seen) = versions[k].clone();
        let (writer, theirs) = open(&bytes, &KEY).unwrap();
        assert_eq!(writer, self.replicas[from].id);
        let r = &mut self.replicas[to];
        r.state.merge(&theirs).unwrap();
        r.seen.extend(seen);
    }

    /// Every replica publishes, then reads everyone's latest file.
    fn gossip(&mut self) {
        let n = self.replicas.len();
        for i in 0..n {
            self.publish(i);
        }
        for to in 0..n {
            for from in 0..n {
                if from != to {
                    let (bytes, seen) = self.files[from].last().unwrap().clone();
                    let (_, theirs) = open(&bytes, &KEY).unwrap();
                    self.replicas[to].state.merge(&theirs).unwrap();
                    self.replicas[to].seen.extend(seen);
                }
            }
        }
    }

    fn run(&mut self, steps: usize) {
        let n = self.replicas.len();
        // The first fetch is complete (no cursor yet) and claims the role.
        assert!(self.fetch(0, false));
        for step in 0..steps {
            self.time += self.rng.random_range(1..30_000);
            if self.handover_at == Some(step) {
                // Possibly before replica 1 has heard of replica 0's claim:
                // then the two claims are concurrent and the stamps decide.
                self.take_fetcher(1);
            }
            let i = self.rng.random_range(0..n);
            match self.rng.random_range(0..100) {
                0..=29 => self.bank.step(&mut self.rng),
                30..=37 if self.replicas[i].fetcher => {
                    self.fetch(i, false);
                }
                38..=54 => self.edit(i),
                55..=69 => self.publish(i),
                70..=97 => {
                    let from = self.rng.random_range(0..n);
                    self.deliver(from, i);
                }
                _ => {
                    // The wall clock jumps back (an NTP step, a manual change).
                    self.replicas[i].skew -= self.rng.random_range(1..60) * MIN;
                }
            }
        }
    }
}

// ------------------------------------------------------------- the checks

fn forecast(state: &SyncState) -> Result<String, String> {
    let (feed, _) = state.materialize()?;
    let feed = serde_json::to_string(&feed).unwrap();
    let options = serde_json::to_string(&state.forecast_options()).unwrap();
    forecast_json(&feed, &options)
}

fn all_identical(w: &World, seed: u64, when: &str) {
    let first = &w.replicas[0].state;
    let bytes = first.to_json();
    let feed = first
        .materialize()
        .map(|(f, _)| serde_json::to_string(&f).unwrap());
    let report = forecast(first);
    assert!(
        report.is_ok(),
        "seed {seed} {when}: forecast failed: {report:?}"
    );
    for r in &w.replicas[1..] {
        assert!(
            r.state.to_json() == bytes,
            "seed {seed} {when}: states differ"
        );
        let theirs = r
            .state
            .materialize()
            .map(|(f, _)| serde_json::to_string(&f).unwrap());
        assert!(theirs == feed, "seed {seed} {when}: feeds differ");
        assert!(
            forecast(&r.state) == report,
            "seed {seed} {when}: forecasts differ"
        );
    }
}

fn intent_holds(w: &World, seed: u64) {
    let state = &w.replicas[0].state;
    let mut by_key: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, e) in w.edits.iter().enumerate() {
        by_key.entry(&e.key).or_default().push(i);
    }
    for (key, ids) in by_key {
        // Causality beats clocks (design §2.5, guarantee 1).
        for &a in &ids {
            for &b in &ids {
                if w.edits[b].deps.contains(&a) {
                    assert!(
                        w.edits[b].stamp > w.edits[a].stamp,
                        "seed {seed}: edit {b} on {key} followed edit {a} but stamps {:?} <= {:?}",
                        w.edits[b].stamp,
                        w.edits[a].stamp
                    );
                }
            }
        }
        // The surviving value is the edit with the largest stamp.
        let winner = ids.iter().max_by_key(|&&i| w.edits[i].stamp).unwrap();
        assert_eq!(
            register(state, key),
            w.edits[*winner].register,
            "seed {seed}: {key} does not hold its newest edit"
        );
    }
}

fn matches_the_bank(w: &World, seed: u64) {
    let state = &w.replicas[0].state;
    let (today, live) = state.live_records().unwrap();
    assert_eq!(today, Date(w.bank.today), "seed {seed}");
    let live: BTreeMap<&str, &BankRecord> = live.into_iter().map(|r| (r.id.as_str(), r)).collect();
    let truth: BTreeMap<&str, &BankRecord> = w
        .bank
        .records
        .iter()
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    if live != truth {
        let ids: BTreeSet<&str> = live.keys().chain(truth.keys()).copied().collect();
        let diff: Vec<String> = ids
            .into_iter()
            .filter(|id| live.get(id) != truth.get(id))
            .map(|id| {
                format!(
                    "{id}: synced {:?}\n  bank {:?}",
                    live.get(id),
                    truth.get(id)
                )
            })
            .collect();
        panic!(
            "seed {seed}: live records differ from the bank's:\n{}",
            diff.join("\n")
        );
    }
    // The feed is exactly the one built from the bank's own records.
    let records: Vec<BankRecord> = w.bank.records.values().cloned().collect();
    let direct = tag::to_feed_with(&records, &w.bank.accounts(), today, |r, p, k| {
        state.effective_kind(r, p, k)
    })
    .unwrap();
    assert!(
        state.materialize().unwrap() == direct,
        "seed {seed}: feed differs"
    );
}

/// How the bank side is set up.
#[derive(Clone, Copy, Debug)]
enum Fetchers {
    /// The approved setup (design §1.4): one device fetches. Its stamps
    /// rise with its own fetches, so the newest bank state always wins and
    /// an ordinary delta fetch suffices at the end.
    One,
    /// Replica 1 takes the fetcher role over at a random step (a new Mac).
    /// Replica 0 keeps fetching until it hears of the handover, then is
    /// refused. Fetches in that window are concurrent and ordered by stamp,
    /// not by which saw the bank later (§1.3), so the new fetcher's first
    /// fetch is complete (it has no cursor) and settles them; the final
    /// fetch at the end is complete too.
    Handover,
}

/// One run: (edits made, records synced, fetches refused).
fn simulate(seed: u64, fetchers: Fetchers) -> (usize, usize, usize) {
    let mut w = World::new(seed, 4, fetchers);
    w.run(400);
    // Quiescence: two rounds of all-to-all exchange.
    w.gossip();
    w.gossip();
    let tag = format!("{fetchers:?}");
    all_identical(&w, seed, &format!("{tag}, after gossip"));
    intent_holds(&w, seed);
    // A fetch by the fetcher after everything has been exchanged settles
    // the bank side, then everyone hears of it.
    let f = w.fetcher();
    let complete = matches!(fetchers, Fetchers::Handover);
    assert!(w.fetch(f, complete), "seed {seed}: the fetcher was refused");
    // Every other device is refused now that all agree on the fetcher.
    for i in (0..w.replicas.len()).filter(|&i| i != f) {
        assert!(!w.fetch(i, false), "seed {seed}: replica {i} fetched");
    }
    w.gossip();
    all_identical(&w, seed, &format!("{tag}, after the final fetch"));
    intent_holds(&w, seed);
    matches_the_bank(&w, seed);
    (w.edits.len(), w.replicas[0].state.records.len(), w.refused)
}

fn many(seeds: std::ops::Range<u64>, fetchers: Fetchers) {
    let runs = seeds.end - seeds.start;
    let (mut edits, mut records, mut refused) = (0, 0, 0);
    for seed in seeds {
        let (e, r, x) = simulate(seed, fetchers);
        edits += e;
        records += r;
        refused += x;
    }
    // The runs did real work: edits made, records synced, and (beyond the
    // checks at the end) the one-fetcher rule refusing fetches mid-run.
    assert!(edits as u64 > runs * 30, "{edits} edits");
    assert!(records as u64 > runs * 150, "{records} records");
    let mid_run = refused as u64 - runs * 3;
    if matches!(fetchers, Fetchers::Handover) {
        assert!(mid_run > runs, "{mid_run} mid-run refusals");
    }
}

#[test]
fn one_fetcher_converges_to_the_bank_with_delta_fetches() {
    many(0..100, Fetchers::One);
}

#[test]
fn a_fetcher_handover_converges_and_the_old_fetcher_is_refused() {
    many(0..100, Fetchers::Handover);
}

#[test]
#[ignore = "soak: thousands of seeds; run with --ignored"]
fn soak() {
    many(1_000..3_000, Fetchers::One);
    many(1_000..3_000, Fetchers::Handover);
}

#[test]
#[ignore = "reruns the seed in $SEED for both setups"]
fn one_seed() {
    let seed = std::env::var("SEED").expect("SEED").parse().unwrap();
    simulate(seed, Fetchers::One);
    simulate(seed, Fetchers::Handover);
}
