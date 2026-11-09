#!/usr/bin/env python3
"""Build/check the pinned real Sputnik DAO test artifact (never a release)."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parent.parent
PINS = ROOT / "gateway/testing/fixtures/sputnik-dao.json"
FIXTURE = ROOT / "target/test-fixtures/sputnik-dao"
WASM = FIXTURE / "sputnikdao2.wasm"


def run(*args, cwd=None):
    return subprocess.run(args, cwd=cwd, check=True, text=True, capture_output=True).stdout.strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_wasm(path, expected):
    if path.read_bytes()[:8] != b"\x00asm\x01\x00\x00\x00":
        raise ValueError(f"not a WebAssembly module: {path}")
    actual = digest(path)
    if actual != expected:
        raise ValueError(f"Sputnik fixture checksum mismatch: expected {expected}, got {actual}")


def source_checkout(pins, source):
    if not source.exists():
        # Reuse Cargo's exact git checkout when available; a cold machine fetches
        # this commit, never a branch, tag, release asset, or mutable latest WASM.
        cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
        for candidate in (cargo_home / "git/checkouts").glob("sputnik-dao-contract-*/*"):
            if run("git", "rev-parse", "HEAD", cwd=candidate) == pins["revision"]:
                subprocess.run(
                    ["git", "clone", "--no-hardlinks", "--no-checkout", str(candidate), str(source)],
                    check=True,
                )
                break
        else:
            source.mkdir()
            run("git", "init", str(source))
            run("git", "remote", "add", "origin", pins["repository"], cwd=source)
            subprocess.run(
                ["git", "-C", str(source), "fetch", "--depth=1", "origin", pins["revision"]],
                check=True,
            )
        run("git", "remote", "set-url", "origin", pins["repository"], cwd=source)
        run("git", "checkout", "--detach", pins["revision"], cwd=source)
    if run("git", "rev-parse", "HEAD", cwd=source) != pins["revision"]:
        raise ValueError(f"wrong Sputnik source revision in {source}")
    if run("git", "status", "--porcelain", "--untracked-files=all", cwd=source):
        raise ValueError(f"Sputnik source checkout must be clean: {source}")
    for name, pin in [("Cargo.lock", "cargo_lock_sha256"), ("rust-toolchain.toml", "rust_toolchain_sha256")]:
        if digest(source / name) != pins[pin]:
            raise ValueError(f"Sputnik {name} does not match fixture pin")
    manifest = tomllib.loads((source / "sputnikdao2/Cargo.toml").read_text())
    recipe = manifest["package"]["metadata"]["near"]["reproducible_build"]
    expected = {
        "image": pins["build_image"],
        "image_digest": pins["build_image_digest"],
        "passed_env": [],
        "container_build_command": pins["container_build_command"],
    }
    if recipe != expected:
        raise ValueError("Sputnik reproducible build metadata does not match fixture pins")
    return source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify existing bytes only; never build or fetch")
    parser.add_argument("--rebuild", action="store_true", help="rebuild even when the verified fixture exists")
    args = parser.parse_args()
    if args.check and args.rebuild:
        parser.error("--check and --rebuild are mutually exclusive")
    pins = json.loads(PINS.read_text())
    expected = pins["wasm_sha256"]
    if len(expected) != 64 or any(char not in "0123456789abcdef" for char in expected):
        raise ValueError("fixture manifest must contain a recorded SHA-256 pin")
    FIXTURE.mkdir(parents=True, exist_ok=True)
    with (FIXTURE / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if WASM.exists() and not args.rebuild:
            verify_wasm(WASM, expected)
        elif args.check:
            raise ValueError(f"missing Sputnik fixture: run just build-sputnik-fixture ({WASM})")
        else:
            version = run("cargo", "near", "--version")
            if version not in (
                f"cargo-near {pins['cargo_near_version']}",
                f"cargo-near-near {pins['cargo_near_version']}",
            ):
                raise ValueError(f"expected cargo-near {pins['cargo_near_version']}, got {version}")
            # cargo-near discovers the outermost Git repository. Keep the
            # upstream checkout outside this worktree so it builds Sputnik,
            # not this repository's commit.
            with (
                tempfile.TemporaryDirectory(prefix="templar-sputnik-") as temporary,
                tempfile.TemporaryDirectory(prefix="build-", dir=FIXTURE) as output_directory,
            ):
                source = source_checkout(pins, Path(temporary) / "source")
                output = Path(output_directory)
                subprocess.run(
                    ["cargo", "near", "build", "reproducible-wasm", "--out-dir", str(output)],
                    cwd=source / "sputnikdao2",
                    check=True,
                )
                candidate = output / "sputnikdao2.wasm"
                verify_wasm(candidate, expected)
                # Verify the locked source stayed untouched before publication.
                source_checkout(pins, source)
                os.replace(candidate, WASM)
    print(f"Sputnik {pins['revision']}: {expected}  {WASM}", file=sys.stderr)


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Sputnik fixture: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr, file=sys.stderr)
        sys.exit(1)
