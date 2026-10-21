#!/usr/bin/env python3
"""Fetch a Plaid Sandbox item's raw data into data/sandbox/ (gitignored).

Creates a fresh sandbox item for a test user, exchanges its public token,
then saves the raw JSON responses of:

  accounts_balance.json   /accounts/balance/get
  transactions_sync.json  /transactions/sync, every page in order
  recurring.json          /transactions/recurring/get (or the error Plaid sent)
  fetch_meta.json         user, institution, local fetch date

`ephemeris import-plaid` turns these into a schema-v1 feed.

Credentials come from .env (PLAID_CLIENT_ID, PLAID_SECRET, PLAID_ENV=sandbox)
and are sent only to Plaid. They are never printed or written. The access
token stays in memory and is never saved; each run creates a new item.

Standard library only (Python 3.9+), so there is nothing to install or pin.

Usage: scripts/fetch_plaid_sandbox.py [--user user_transactions_dynamic]
"""

import argparse
import datetime
import json
import pathlib
import sys
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "data" / "sandbox"
BASE_URL = "https://sandbox.plaid.com"
# First Platypus Bank: non-OAuth, so special test credentials apply.
INSTITUTION = "ins_109508"
DAYS_REQUESTED = 730
SYNC_WAIT_SECONDS = 180


class PlaidError(Exception):
    def __init__(self, path, status, body):
        self.path, self.status, self.body = path, status, body
        code = body.get("error_code", "?") if isinstance(body, dict) else "?"
        msg = body.get("error_message", "") if isinstance(body, dict) else ""
        super().__init__(f"{path}: HTTP {status} {code} {msg}".strip())


def load_env(path):
    env = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        env[key.strip()] = value.strip().strip("'\"")
    return env


def make_client(client_id, secret):
    def post(path, body):
        req = urllib.request.Request(
            BASE_URL + path,
            data=json.dumps(body).encode(),
            headers={
                "Content-Type": "application/json",
                "PLAID-CLIENT-ID": client_id,
                "PLAID-SECRET": secret,
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                return json.load(resp)
        except urllib.error.HTTPError as e:
            try:
                body = json.load(e)
            except ValueError:
                body = {}
            raise PlaidError(path, e.code, body) from None

    return post


def save(name, data):
    path = OUT / name
    path.write_text(json.dumps(data, indent=2) + "\n")
    print(f"  wrote {path.relative_to(ROOT)}")


def sync_all(post, token):
    """All /transactions/sync pages from the start, once history is ready."""
    deadline = time.monotonic() + SYNC_WAIT_SECONDS
    delay = 2.0
    while True:
        pages, cursor = [], None
        while True:
            body = {"access_token": token, "count": 500}
            if cursor:
                body["cursor"] = cursor
            page = post("/transactions/sync", body)
            pages.append(page)
            cursor = page["next_cursor"]
            if not page["has_more"]:
                break
        status = pages[-1].get("transactions_update_status")
        if status == "HISTORICAL_UPDATE_COMPLETE" or time.monotonic() > deadline:
            return pages, status
        print(f"  transactions not ready ({status}); retrying in {delay:.0f}s")
        time.sleep(delay)
        delay = min(delay * 2, 30.0)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--user", default="user_transactions_dynamic")
    args = parser.parse_args()

    env_path = ROOT / ".env"
    if not env_path.exists():
        sys.exit("error: .env not found (needs PLAID_CLIENT_ID, PLAID_SECRET, PLAID_ENV)")
    env = load_env(env_path)
    missing = [k for k in ("PLAID_CLIENT_ID", "PLAID_SECRET") if not env.get(k)]
    if missing:
        sys.exit(f"error: .env is missing {', '.join(missing)}")
    if env.get("PLAID_ENV", "sandbox") != "sandbox":
        sys.exit("error: this script only talks to Plaid Sandbox (PLAID_ENV=sandbox)")

    post = make_client(env["PLAID_CLIENT_ID"], env["PLAID_SECRET"])
    OUT.mkdir(parents=True, exist_ok=True)
    try:
        print(f"Creating sandbox item for {args.user} at {INSTITUTION}")
        public = post(
            "/sandbox/public_token/create",
            {
                "institution_id": INSTITUTION,
                "initial_products": ["transactions"],
                "options": {
                    "override_username": args.user,
                    "override_password": "pass_good",
                    "transactions": {"days_requested": DAYS_REQUESTED},
                },
            },
        )
        token = post(
            "/item/public_token/exchange", {"public_token": public["public_token"]}
        )["access_token"]

        print("Syncing transactions")
        pages, status = sync_all(post, token)
        added = sum(len(p["added"]) for p in pages)
        print(f"  {len(pages)} page(s), {added} transactions, status {status}")
        save("transactions_sync.json", {"pages": pages})

        print("Fetching balances")
        save("accounts_balance.json", post("/accounts/balance/get", {"access_token": token}))

        print("Fetching recurring streams")
        try:
            recurring = post("/transactions/recurring/get", {"access_token": token})
        except PlaidError as e:
            print(f"  recurring unavailable: {e}")
            recurring = {"error": e.body, "http_status": e.status}
        save("recurring.json", recurring)

        save(
            "fetch_meta.json",
            {
                "user": args.user,
                "institution_id": INSTITUTION,
                "days_requested": DAYS_REQUESTED,
                "fetched_on": datetime.date.today().isoformat(),
                "transactions_update_status": status,
            },
        )
    except PlaidError as e:
        sys.exit(f"error: {e}")
    except urllib.error.URLError as e:
        sys.exit(f"error: cannot reach Plaid: {e.reason}")


if __name__ == "__main__":
    main()
