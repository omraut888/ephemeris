#!/usr/bin/env python3
"""The fetch script never passes the Plaid access token in a process's
arguments (docs/phase4b-design.md §1.4).

A stand-in `security`, first on PATH, records exactly the argv and stdin it
receives and behaves like the real tool for the three commands the script
uses. The token must appear only on stdin (storing) and stdout (reading).
Nothing here talks to Plaid or the real Keychain.

Run: python3 -I scripts/test_fetch_plaid_sandbox.py (scripts/test-full.sh does).
"""

import importlib.util
import os
import pathlib
import stat
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent

SHIM = """#!/bin/sh
for a in "$@"; do printf '[%s]' "$a"; done >> "{log}/argv.log"
echo >> "{log}/argv.log"
case "$1" in
  -i) tee -a "{log}/stdin.log" | sed -n 's/.*-w "\\([^"]*\\)".*/\\1/p' > "{log}/store" ;;
  find-generic-password) [ -s "{log}/store" ] || exit 44; cat "{log}/store" ;;
  delete-generic-password) rm -f "{log}/store" ;;
esac
"""


def load_script():
    spec = importlib.util.spec_from_file_location("fetch", HERE / "fetch_plaid_sandbox.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TokenNeverInArgv(unittest.TestCase):
    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp(prefix="ephemeris-security-shim-"))
        shim = self.dir / "security"
        shim.write_text(SHIM.format(log=self.dir))
        shim.chmod(shim.stat().st_mode | stat.S_IEXEC)
        self.path = os.environ["PATH"]
        os.environ["PATH"] = f"{self.dir}{os.pathsep}{self.path}"
        self.fetch = load_script()

    def tearDown(self):
        os.environ["PATH"] = self.path
        for f in self.dir.iterdir():
            f.unlink()
        self.dir.rmdir()

    def test_store_load_and_delete_keep_the_token_out_of_argv(self):
        item = "D6z41wp4K9iE3l3jlGx5uQ9EQKj8dBSzKywW8"
        token = "access-sandbox-1b2e7c4a-9f0d-4e55-8a31-3c9d2f6b7e10"
        self.fetch.keychain_store(item, token)
        self.assertEqual(self.fetch.keychain_load(item), token)
        self.fetch.keychain_delete(item)
        self.assertIsNone(self.fetch.keychain_load(item))

        argv = (self.dir / "argv.log").read_text()
        stdin = (self.dir / "stdin.log").read_text()
        self.assertEqual(
            argv.splitlines(),
            [
                "[-i]",
                f"[find-generic-password][-s][ephemeris.plaid][-a][{item}][-w]",
                f"[find-generic-password][-s][ephemeris.plaid][-a][{item}][-w]",
                f"[delete-generic-password][-s][ephemeris.plaid][-a][{item}]",
                f"[find-generic-password][-s][ephemeris.plaid][-a][{item}][-w]",
            ],
        )
        self.assertNotIn(token, argv)
        for part in token.split("-")[2:]:
            self.assertNotIn(part, argv)
        self.assertIn(f'-w "{token}"', stdin)

    def test_unexpected_characters_never_reach_security(self):
        with self.assertRaises(SystemExit):
            self.fetch.keychain_store("item", 'tok" ; delete-keychain "x')
        self.assertFalse((self.dir / "argv.log").exists())


if __name__ == "__main__":
    unittest.main()
