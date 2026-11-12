#!/usr/bin/env python3
"""Fetch/check the hash-pinned external Sputnik DAO test fixture."""

import argparse
import fcntl
import hashlib
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
PINS = ROOT / "gateway/testing/fixtures/sputnik-dao.json"
FIXTURE = ROOT / "target/test-fixtures/sputnik-dao"
WASM = FIXTURE / "sputnikdao2.wasm"


def verify_wasm(bytes_, expected, length):
    if len(bytes_) != length:
        raise ValueError(f"Sputnik fixture length mismatch: expected {length}, got {len(bytes_)}")
    if bytes_[:8] != b"\x00asm\x01\x00\x00\x00":
        raise ValueError("not a WebAssembly 1.0 module")
    actual = hashlib.sha256(bytes_).hexdigest()
    if actual != expected:
        raise ValueError(f"Sputnik fixture checksum mismatch: expected {expected}, got {actual}")


def fetch_wasm(pins):
    subprocess.run(
        [
            "cargo", "run", "--quiet", "-p", "templar-contract-artifacts",
            "--features", "fetch,clap", "--bin", "fetch-artifacts", "--",
            "download", "--url", pins["release_url"], "--sha256", pins["wasm_sha256"],
            "--length", str(pins["wasm_length"]), "--output", str(WASM),
        ],
        cwd=ROOT,
        check=True,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="verify cached bytes only; never use the network")
    mode.add_argument("--refetch", action="store_true", help="download the pinned release again, even if cached")
    args = parser.parse_args()
    pins = json.loads(PINS.read_text())
    expected = pins["wasm_sha256"]
    if len(expected) != 64 or any(char not in "0123456789abcdef" for char in expected):
        raise ValueError("fixture manifest must contain a recorded SHA-256 pin")
    FIXTURE.mkdir(parents=True, exist_ok=True)
    with (FIXTURE / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if WASM.exists() and not args.refetch:
            verify_wasm(WASM.read_bytes(), expected, pins["wasm_length"])
        elif args.check:
            raise ValueError(f"missing Sputnik fixture: run just fetch-sputnik-fixture ({WASM})")
        else:
            fetch_wasm(pins)
    print(f"Sputnik {pins['account_id']}: {expected}  {WASM}", file=sys.stderr)


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Sputnik fixture: {error}", file=sys.stderr)
        sys.exit(1)
