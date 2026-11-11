#!/usr/bin/env python3
"""Fetch/check the hash-pinned canonical Sputnik DAO test artifact (never a release)."""

import argparse
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parent.parent
PINS = ROOT / "gateway/testing/fixtures/sputnik-dao.json"
FIXTURE = ROOT / "target/test-fixtures/sputnik-dao"
WASM = FIXTURE / "sputnikdao2.wasm"


def verify_wasm(bytes_, expected):
    if bytes_[:8] != b"\x00asm\x01\x00\x00\x00":
        raise ValueError("not a WebAssembly 1.0 module")
    actual = hashlib.sha256(bytes_).hexdigest()
    if actual != expected:
        raise ValueError(f"Sputnik fixture checksum mismatch: expected {expected}, got {actual}")


def fetch_wasm(pins):
    request = Request(
        pins["rpc_url"],
        data=json.dumps({
            "jsonrpc": "2.0",
            "id": "sputnik-fixture",
            "method": "query",
            "params": {
                "request_type": "view_code",
                "account_id": pins["account_id"],
                "finality": "final",
            },
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urlopen(request, timeout=60) as response:
        payload = json.load(response)
    if "error" in payload:
        raise ValueError(f"view_code failed: {payload['error']}")
    result = payload["result"]
    if result["hash"] != pins["code_hash"]:
        raise ValueError(
            f"canonical DAO code hash changed: expected {pins['code_hash']}, got {result['hash']}"
        )
    return base64.b64decode(result["code_base64"], validate=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="verify cached bytes only; never use the network")
    mode.add_argument("--refetch", action="store_true", help="fetch current finalized code even if cached; keep the same pins")
    args = parser.parse_args()
    pins = json.loads(PINS.read_text())
    expected = pins["wasm_sha256"]
    if len(expected) != 64 or any(char not in "0123456789abcdef" for char in expected):
        raise ValueError("fixture manifest must contain a recorded SHA-256 pin")
    FIXTURE.mkdir(parents=True, exist_ok=True)
    with (FIXTURE / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if WASM.exists() and not args.refetch:
            verify_wasm(WASM.read_bytes(), expected)
        elif args.check:
            raise ValueError(f"missing Sputnik fixture: run just fetch-sputnik-fixture ({WASM})")
        else:
            bytes_ = fetch_wasm(pins)
            verify_wasm(bytes_, expected)
            # Verify before touching the cache; failed fetches never replace it.
            with tempfile.TemporaryDirectory(prefix="fetch-", dir=FIXTURE) as temporary:
                candidate = Path(temporary) / WASM.name
                candidate.write_bytes(bytes_)
                os.replace(candidate, WASM)
    print(f"Sputnik {pins['account_id']}: {expected}  {WASM}", file=sys.stderr)


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, ValueError) as error:
        print(f"Sputnik fixture: {error}", file=sys.stderr)
        sys.exit(1)
