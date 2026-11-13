#!/usr/bin/env python3
"""Fetch the paper's public datasets and check them against pinned SHA-256s.

    paper/.venv/bin/python -I scripts/fetch_paper_data.py [--print-hashes]

Raw files are never committed (data/berka/ and data/moneydata/ are
gitignored). Each dataset is written in a canonical form, so its hash is
stable whatever the server's row order:

- Berka (PKDD'99 financial data), from the CTU Relational Learning
  Repository's public MariaDB server (guest login, as published at
  https://relational.fel.cvut.cz/dataset/Financial). Every table becomes
  data/berka/<table>.tsv: a header line, rows sorted by the table's id,
  tab-separated, NULL written as \\N, dates as YYYY-MM-DD.
- MoneyData (Firat et al., EuroVis 2023; CC BY 4.0), the one XLSX file of
  Mendeley Data DOI 10.17632/dnxtg6n4rv.1. The download is checked
  against the SHA-256 Mendeley publishes, then its first sheet becomes
  data/moneydata/transactions.tsv with the same conventions.

Without --print-hashes the script fails if any file differs from
paper/data.sha256. With it, the script prints the hashes instead (used
once, to pin them).
"""

import argparse
import datetime
import decimal
import hashlib
import pathlib
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
PINS = ROOT / "paper" / "data.sha256"

BERKA = dict(host="relational.fel.cvut.cz", port=3306, user="guest",
             password="ctu-relational", database="financial")
BERKA_TABLES = {  # table -> sort key
    "account": "account_id", "card": "card_id", "client": "client_id",
    "disp": "disp_id", "district": "district_id", "loan": "loan_id",
    "order": "order_id", "trans": "trans_id",
}

MONEYDATA_URL = ("https://data.mendeley.com/public-files/datasets/dnxtg6n4rv/"
                 "files/36b3d359-2491-4282-850f-3f03e00fbfe3/file_downloaded")
MONEYDATA_XLSX_SHA256 = ("716eaf9ad4a3cf60dc9c9c8a20ed9a917c545a1b5f139f40ed91d2104faa3ff7")


def cell(v):
    if v is None:
        return "\\N"
    if isinstance(v, datetime.datetime):
        if v.time() == datetime.time(0):
            return v.date().isoformat()
        return v.isoformat()
    if isinstance(v, datetime.date):
        return v.isoformat()
    if isinstance(v, decimal.Decimal):
        return format(v, "f")
    s = str(v)
    if "\t" in s or "\n" in s:
        raise ValueError(f"tab or newline in a value: {s!r}")
    return s


def write_tsv(path, header, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write("\t".join(header) + "\n")
        for r in rows:
            f.write("\t".join(cell(v) for v in r) + "\n")


def fetch_berka(out):
    import pymysql

    conn = pymysql.connect(connect_timeout=30, read_timeout=600, **BERKA)
    try:
        cur = conn.cursor()
        for table, key in BERKA_TABLES.items():
            cur.execute(f"SELECT * FROM `{table}` ORDER BY `{key}`")
            header = [d[0] for d in cur.description]
            rows = cur.fetchall()
            write_tsv(out / f"{table}.tsv", header, rows)
            print(f"berka/{table}.tsv: {len(rows)} rows", file=sys.stderr)
    finally:
        conn.close()


def fetch_moneydata(out):
    import openpyxl

    out.mkdir(parents=True, exist_ok=True)
    xlsx = out / "moneydata.xlsx"
    req = urllib.request.Request(MONEYDATA_URL, headers={"User-Agent": "ephemeris-fetch"})
    with urllib.request.urlopen(req, timeout=120) as r:
        data = r.read()
    got = hashlib.sha256(data).hexdigest()
    if got != MONEYDATA_XLSX_SHA256:
        sys.exit(f"MoneyData download hash {got} != published {MONEYDATA_XLSX_SHA256}")
    xlsx.write_bytes(data)
    wb = openpyxl.load_workbook(xlsx, read_only=True, data_only=True)
    ws = wb.worksheets[0]
    rows = [r for r in ws.iter_rows(values_only=True) if any(v is not None for v in r)]
    header = [cell(v) for v in rows[0]]
    write_tsv(out / "transactions.tsv", header, rows[1:])
    print(f"moneydata/transactions.tsv: {len(rows) - 1} rows, sheet {ws.title!r}",
          file=sys.stderr)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--print-hashes", action="store_true")
    ap.add_argument("--only", choices=["berka", "moneydata"])
    args = ap.parse_args()

    if args.only in (None, "berka"):
        fetch_berka(ROOT / "data" / "berka")
    if args.only in (None, "moneydata"):
        fetch_moneydata(ROOT / "data" / "moneydata")

    files = sorted((ROOT / "data").glob("berka/*.tsv")) + \
        sorted((ROOT / "data").glob("moneydata/*.tsv"))
    lines = [f"{sha256(p)}  {p.relative_to(ROOT / 'data')}" for p in files]
    if args.print_hashes:
        print("\n".join(lines))
        return
    pinned = {l.split()[1]: l.split()[0] for l in PINS.read_text().splitlines() if l.strip()}
    bad = []
    for line in lines:
        h, name = line.split()
        if name in pinned and pinned[name] != h:
            bad.append(f"{name}: {h} != pinned {pinned[name]}")
    missing = set(pinned) - {l.split()[1] for l in lines}
    if args.only:
        missing = {m for m in missing if m.startswith(args.only)}
    bad += [f"{m}: missing" for m in sorted(missing)]
    if bad:
        sys.exit("checksum mismatch:\n  " + "\n  ".join(bad))
    print("all datasets match paper/data.sha256", file=sys.stderr)


if __name__ == "__main__":
    main()
