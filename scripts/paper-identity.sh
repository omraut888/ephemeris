#!/usr/bin/env bash
# Bit identity on this Mac (paper/PLAN.md §1 claim C1, T10): the golden
# feed plus the 100 generated cases (crates/ephemeris-wasm/tests/common/
# identity_cases.rs), on native arm64 (debug and release), UniFFI (the
# exported functions, native) and WASM under Node. Writes
# paper/results/identity/identity.csv. x86_64 is covered by the GitHub
# Actions workflow for the golden feed only (PLAN §11).
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=paper/results/identity
LOG=paper/build/identity
mkdir -p "$OUT" "$LOG"

# Each leg: its name, then the command. A leg's log is kept for the counts.
run() {
  local name=$1; shift
  if "$@" >"$LOG/$name.log" 2>&1; then echo "$name,pass"; else echo "$name,fail"; fi
}

{
  echo "leg,golden,identity"
  g=$(run native-debug-golden cargo test -q -p ephemeris-wasm --test golden)
  i=$(run native-debug-identity cargo test -q -p ephemeris-wasm --test identity)
  echo "native arm64 debug,${g#*,},${i#*,}"
  g=$(run native-release-golden cargo test -q --release -p ephemeris-wasm --test golden)
  i=$(run native-release-identity cargo test -q --release -p ephemeris-wasm --test identity)
  echo "native arm64 release,${g#*,},${i#*,}"
  g=$(run ffi-golden cargo test -q -p ephemeris-ffi exported_functions_match_the_golden_files)
  i=$(run ffi-identity cargo test -q -p ephemeris-ffi exported_functions_match_the_identity_manifest)
  echo "uniffi (native arm64),${g#*,},${i#*,}"
  g=$(run wasm-golden wasm-pack test --node --release crates/ephemeris-wasm --test wasm)
  i=$(run wasm-identity wasm-pack test --node --release crates/ephemeris-wasm --test identity -- --include-ignored)
  echo "wasm32 under node,${g#*,},${i#*,}"
} >"$LOG/legs.csv"

# Per leg: cases whose feed and report hashes match the manifest.
paper/.venv/bin/python -I - "$LOG" "$OUT" <<'EOF'
import csv, pathlib, re, subprocess, sys
log, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
cases = 100
def counts(name):
    text = (log / f"{name}.log").read_text()
    feeds = reports = 0
    for exp, got in re.findall(r"expected (\d+\\t\w+\\t\w+) / got (\d+\\t\w+\\t\w+)", text):
        e, g = exp.split("\\t"), got.split("\\t")
        feeds += e[1] != g[1]
        reports += e[2] != g[2]
    return feeds, reports
rows = []
for leg in csv.DictReader(open(log / "legs.csv")):
    key = {"native arm64 debug": "native-debug-identity",
           "native arm64 release": "native-release-identity",
           "uniffi (native arm64)": "ffi-identity",
           "wasm32 under node": "wasm-identity"}[leg["leg"]]
    if leg["identity"] == "pass":
        fd = rd = 0
    else:
        fd, rd = counts(key)
        if fd == rd == 0:
            sys.exit(f"{key}: failed without a parsable difference; see {log}/{key}.log")
    rows.append([leg["leg"], leg["golden"], cases, cases - fd, cases - rd])
tool = lambda *a: subprocess.run(a, capture_output=True, text=True).stdout.strip()
with open(out / "identity.csv", "w") as f:
    w = csv.writer(f, lineterminator="\n")
    w.writerow(["leg", "golden_feed", "cases", "feeds_identical", "reports_identical"])
    w.writerows(rows)
with open(out / "toolchain.csv", "w") as f:
    w = csv.writer(f, lineterminator="\n")
    w.writerow(["tool", "version"])
    w.writerow(["rustc", tool("rustc", "--version")])
    w.writerow(["wasm-pack", tool("wasm-pack", "--version")])
    w.writerow(["node", tool("node", "--version")])
    w.writerow(["machine", tool("uname", "-m")])
print((out / "identity.csv").read_text())
EOF
