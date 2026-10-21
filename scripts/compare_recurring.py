#!/usr/bin/env python3
"""Compare our detected schedule with Plaid's /transactions/recurring/get.

Reads, from data/sandbox/ by default:
  recurring.json, accounts_balance.json   (scripts/fetch_plaid_sandbox.py)
  report.json                             (ephemeris forecast --json)

and prints a Markdown table of matches, misses (Plaid found it, we did not)
and false positives (we found it, Plaid did not). Plaid streams on
non-depository accounts are listed separately: Phase 1 ingests checking and
savings only, so they are out of scope rather than misses.

Matching is by counterparty name (case and punctuation ignored, either name
containing the other). Standard library only.

Usage: scripts/compare_recurring.py [DIR]
"""

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def norm(s):
    return re.sub(r"[^a-z0-9]+", " ", (s or "").lower()).strip()


def same(a, b):
    a, b = norm(a), norm(b)
    return bool(a and b) and (a in b or b in a)


def our_frequency(item):
    if "cadence" in item:
        return item["cadence"]["kind"].upper()
    if item.get("monthly_on"):
        return "MONTHLY"
    p = item["period_days"]
    for lo, hi, name in ((6, 8, "WEEKLY"), (13, 15, "BIWEEKLY"), (28, 31, "MONTHLY")):
        if lo <= p <= hi:
            return name
    return f"EVERY_{p}_DAYS"


def describe_ours(item):
    if "cadence" in item:
        c = item["cadence"]
        return c["kind"]
    if item.get("monthly_on"):
        on = item["monthly_on"]
        shift = "" if on["shift"] == "none" else f", {on['shift'].replace('_', ' ')}"
        return f"monthly on day {on['day']}{shift}"
    return f"every {item['period_days']} days"


def main():
    d = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "data" / "sandbox"
    recurring = json.loads((d / "recurring.json").read_text())
    if "error" in recurring:
        sys.exit(f"error: Plaid returned no recurring streams: {recurring['error']}")
    accounts = {
        a["account_id"]: a for a in json.loads((d / "accounts_balance.json").read_text())["accounts"]
    }
    report = json.loads((d / "report.json").read_text())

    plaid, out_of_scope = [], []
    for side in ("inflow_streams", "outflow_streams"):
        for s in recurring[side]:
            acct = accounts.get(s["account_id"], {})
            entry = {
                "name": s.get("merchant_name") or s["description"],
                "description": s["description"],
                "frequency": s["frequency"],
                # Plaid: positive = money out. Flip to our convention.
                "amount": -s["last_amount"]["amount"],
                "next": s.get("predicted_next_date"),
                "status": s["status"],
                "account": acct.get("subtype") or acct.get("type", "?"),
            }
            (plaid if acct.get("type") == "depository" else out_of_scope).append(entry)

    ours = []
    if report.get("income"):
        inc = report["income"]
        amount = report["expected_paycheck"]["mean"]
        ours.append({**inc, "kind": "income", "amount": amount, "next": inc.get("next_scheduled")})
    for b in report["bills"]:
        ours.append({**b, "kind": "bill", "next": b["next_due"]})

    rows, used = [], set()
    for p in plaid:
        match = next(
            (
                i
                for i, o in enumerate(ours)
                if i not in used
                and (same(p["name"], o.get("name")) or same(p["description"], o.get("name")))
            ),
            None,
        )
        if match is None:
            rows.append(("miss", p, None))
        else:
            used.add(match)
            rows.append(("match", p, ours[match]))
    for i, o in enumerate(ours):
        if i not in used:
            rows.append(("false positive", None, o))

    print(f"Feed as of {report['as_of']}; Plaid streams on depository accounts only.\n")
    print("| Result | Stream | Plaid | Ours | Amount (Plaid / ours) | Next (Plaid / ours) |")
    print("|---|---|---|---|---|---|")
    for result, p, o in rows:
        name = (p or {}).get("name") or (o or {}).get("name") or f"stream {o.get('stream')}"
        plaid_freq = f"{p['frequency']} ({p['status']})" if p else "-"
        our_freq = describe_ours(o) if o else "-"
        if p and o and our_frequency(o) != p["frequency"]:
            result += ", frequency differs"
        if p and o and abs(p["amount"] - o.get("amount", p["amount"])) > 0.005:
            result += ", amount differs"
        amount = f"{p['amount']:.2f}" if p else "-"
        amount += " / " + (f"{o['amount']:.2f}" if o and "amount" in o else "-")
        nxt = f"{(p or {}).get('next') or '-'} / {(o or {}).get('next') or '-'}"
        if p and o and p.get("next") and o.get("next") and p["next"] != o["next"]:
            result += ", next date differs"
        print(f"| {result} | {name} | {plaid_freq} | {our_freq} | {amount} | {nxt} |")

    if out_of_scope:
        print("\nOut of scope (non-depository accounts, not ingested in Phase 1):\n")
        for p in out_of_scope:
            print(f"- {p['name']}: {p['frequency']}, {p['amount']:.2f}, on the {p['account']}")


if __name__ == "__main__":
    main()
