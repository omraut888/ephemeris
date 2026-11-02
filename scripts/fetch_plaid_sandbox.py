#!/usr/bin/env python3
"""Fetch a Plaid Sandbox item's raw data into data/sandbox/ (gitignored).

The first run creates a sandbox item for a test user and keeps it: its
access token goes to the macOS Keychain, never to a file
(docs/phase4b-design.md §1.4). Later runs fetch the same item
incrementally, from the /transactions/sync cursor the last run saved.

Each run saves the raw JSON responses of:

  accounts_balance.json   /accounts/balance/get
  transactions_sync.json  /transactions/sync, every page in order: from no
                          cursor (a full fetch) or from the saved one (a delta)
  recurring.json          /transactions/recurring/get (or the error Plaid sent)
  fetch_meta.json         user, institution, item, fetch date, mode, and the
                          fetch's sequence number

and keeps fetch_state.json (item id, cursor, sequence; nothing secret).

`ephemeris import-plaid` turns a full fetch into a schema-v1 feed;
`ephemeris sync import-plaid` records either kind in a sync state, and
refuses a delta whose predecessor it never saw.

The token: Keychain generic password, service "ephemeris.plaid", account =
the Plaid item_id. It is written by piping a command into `security -i`, so
it never appears in a process's arguments, and read back with
`security find-generic-password -w`. It is never printed or written to disk.

Credentials come from .env (PLAID_CLIENT_ID, PLAID_SECRET, PLAID_ENV=sandbox)
and are sent only to Plaid. They are never printed or written.

Standard library only (Python 3.9+), so there is nothing to install or pin.

Usage:
  scripts/fetch_plaid_sandbox.py              # new item first time, then deltas
  scripts/fetch_plaid_sandbox.py --refresh    # ask the sandbox for new activity first
  scripts/fetch_plaid_sandbox.py --full       # same item, everything again
  scripts/fetch_plaid_sandbox.py --new-item   # forget the item, start another
  scripts/fetch_plaid_sandbox.py --forget     # remove the item and its token
"""

import argparse
import datetime
import json
import pathlib
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_OUT = ROOT / "data" / "sandbox"
BASE_URL = "https://sandbox.plaid.com"
# First Platypus Bank: non-OAuth, so special test credentials apply.
INSTITUTION = "ins_109508"
DAYS_REQUESTED = 730
SYNC_WAIT_SECONDS = 180
KEYCHAIN_SERVICE = "ephemeris.plaid"
# Plaid ids and tokens are letters, digits, '-' and '_'. Anything else is
# refused before it reaches `security -i`, whose command line it would join.
SAFE = re.compile(r"^[A-Za-z0-9_-]+$")


class PlaidError(Exception):
    def __init__(self, path, status, body):
        self.path, self.status, self.body = path, status, body
        code = body.get("error_code", "?") if isinstance(body, dict) else "?"
        msg = body.get("error_message", "") if isinstance(body, dict) else ""
        self.code = code
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


# ------------------------------------------------------------- the Keychain


def keychain_store(item_id, token):
    if not (SAFE.match(item_id) and SAFE.match(token)):
        sys.exit("error: Plaid returned an item id or token with unexpected characters")
    command = (
        f'add-generic-password -U -s "{KEYCHAIN_SERVICE}" -a "{item_id}" -w "{token}"\n'
    )
    subprocess.run(
        ["security", "-i"],
        input=command,
        text=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if keychain_load(item_id) != token:
        sys.exit("error: could not store the access token in the Keychain")


def keychain_load(item_id):
    """The stored token, or None."""
    r = subprocess.run(
        ["security", "find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", item_id, "-w"],
        capture_output=True,
        text=True,
        check=False,
    )
    return r.stdout.strip() if r.returncode == 0 and r.stdout.strip() else None


def keychain_delete(item_id):
    subprocess.run(
        ["security", "delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", item_id],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


# ------------------------------------------------------------ fetching


def sync_pages(post, token, cursor):
    """Every /transactions/sync page after `cursor` (None: from the start)."""
    for attempt in range(5):
        pages, c = [], cursor
        try:
            while True:
                body = {"access_token": token, "count": 500}
                if c:
                    body["cursor"] = c
                page = post("/transactions/sync", body)
                pages.append(page)
                c = page["next_cursor"]
                if not page["has_more"]:
                    return pages
        except PlaidError as e:
            # Plaid's rule: restart the whole pagination from the same cursor.
            if e.code != "TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION" or attempt == 4:
                raise
            print("  transactions changed while paging; starting over")
    raise AssertionError("unreachable")


def full_sync(post, token):
    """All pages from no cursor, once the item's history is ready."""
    deadline = time.monotonic() + SYNC_WAIT_SECONDS
    delay = 2.0
    while True:
        pages = sync_pages(post, token, None)
        status = pages[-1].get("transactions_update_status")
        if status == "HISTORICAL_UPDATE_COMPLETE" or time.monotonic() > deadline:
            return pages, status
        print(f"  transactions not ready ({status}); retrying in {delay:.0f}s")
        time.sleep(delay)
        delay = min(delay * 2, 30.0)


def new_item(post, user):
    print(f"Creating sandbox item for {user} at {INSTITUTION}")
    public = post(
        "/sandbox/public_token/create",
        {
            "institution_id": INSTITUTION,
            "initial_products": ["transactions"],
            "options": {
                "override_username": user,
                "override_password": "pass_good",
                "transactions": {"days_requested": DAYS_REQUESTED},
            },
        },
    )
    exchanged = post("/item/public_token/exchange", {"public_token": public["public_token"]})
    keychain_store(exchanged["item_id"], exchanged["access_token"])
    print(f"  item {exchanged['item_id']}; access token stored in the Keychain")
    return exchanged["item_id"], exchanged["access_token"]


def forget(post, state, state_path):
    item_id = state.get("item_id") if state else None
    if not item_id:
        print("No saved item.")
        return
    token = keychain_load(item_id)
    if token:
        try:
            post("/item/remove", {"access_token": token})
            print(f"Removed item {item_id} at Plaid")
        except PlaidError as e:
            print(f"  could not remove the item at Plaid ({e}); forgetting it anyway")
    keychain_delete(item_id)
    state_path.unlink(missing_ok=True)
    print(f"Deleted its Keychain entry and {state_path.name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--user", default="user_transactions_dynamic")
    parser.add_argument("--out", type=pathlib.Path, default=DEFAULT_OUT,
                        help="where to write the dumps and fetch_state.json")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--full", action="store_true",
                      help="fetch the saved item from no cursor (everything again)")
    mode.add_argument("--new-item", action="store_true",
                      help="forget the saved item and create a new one")
    mode.add_argument("--forget", action="store_true",
                      help="remove the saved item at Plaid, delete its token, and stop")
    parser.add_argument("--refresh", action="store_true",
                        help="call /transactions/refresh first (the dynamic user then makes new activity)")
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
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    state_path = out / "fetch_state.json"
    state = json.loads(state_path.read_text()) if state_path.exists() else None

    def save(name, data):
        path = out / name
        path.write_text(json.dumps(data, indent=2) + "\n")
        shown = path.relative_to(ROOT) if path.is_relative_to(ROOT) else path
        print(f"  wrote {shown}")

    try:
        if args.forget or args.new_item:
            forget(post, state, state_path)
            state = None
            if args.forget:
                return

        token = keychain_load(state["item_id"]) if state else None
        if state and not token:
            sys.exit(
                f"error: no Keychain token for item {state['item_id']}; "
                "run with --new-item to start another item"
            )
        if not state:
            item_id, token = new_item(post, args.user)
            state = {"item_id": item_id, "user": args.user, "seq": 0,
                     "cursor": None, "fetched_on": None}
            # Saved at once, so a failure below never orphans the token.
            save("fetch_state.json", state)

        if args.refresh:
            print("Asking the sandbox for new activity")
            post("/transactions/refresh", {"access_token": token})
            time.sleep(5)

        today = datetime.date.today().isoformat()
        incremental = bool(state["cursor"]) and not args.full
        if incremental:
            print(f"Syncing transactions since fetch {state['seq']} ({state['fetched_on']})")
            pages = sync_pages(post, token, state["cursor"])
            status = pages[-1].get("transactions_update_status")
        else:
            print("Syncing all transactions")
            pages, status = full_sync(post, token)
        counts = {k: sum(len(p.get(k, [])) for p in pages) for k in ("added", "modified", "removed")}
        print(f"  {len(pages)} page(s): {counts['added']} added, "
              f"{counts['modified']} modified, {counts['removed']} removed; status {status}")
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

        seq = state["seq"] + 1
        meta = {
            "user": state["user"],
            "institution_id": INSTITUTION,
            "days_requested": DAYS_REQUESTED,
            "fetched_on": today,
            "transactions_update_status": status,
            "item_id": state["item_id"],
            "mode": "incremental" if incremental else "full",
            "fetch_seq": seq,
        }
        if incremental:
            meta["previous_seq"] = state["seq"]
            meta["previous_fetched_on"] = state["fetched_on"]
        save("fetch_meta.json", meta)

        state.update(seq=seq, cursor=pages[-1]["next_cursor"], fetched_on=today)
        save("fetch_state.json", state)
        print(f"Fetch {seq} ({meta['mode']}) of item {state['item_id']}")
    except PlaidError as e:
        sys.exit(f"error: {e}")
    except urllib.error.URLError as e:
        sys.exit(f"error: cannot reach Plaid: {e.reason}")


if __name__ == "__main__":
    main()
