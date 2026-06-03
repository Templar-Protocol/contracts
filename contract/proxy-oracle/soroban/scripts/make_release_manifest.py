#!/usr/bin/env python3
"""Build and write a deterministic release manifest for the Soroban proxy-oracle.

Reads both optimized WASM artifacts, computes SHA-256 checksums, collects
package version, git commit, stellar CLI version, rust toolchain, and optimized
sizes, then writes a JSON manifest to:

    target/proxy-oracle-soroban/release-manifest.json

Usage:
    python3 scripts/make_release_manifest.py \\
        --root <workspace_root> \\
        --runtime-wasm <path/to/runtime.optimized.wasm> \\
        --governance-wasm <path/to/governance.optimized.wasm> \\
        --runtime-pkg templar-proxy-oracle-soroban-contract \\
        --governance-pkg templar-proxy-oracle-soroban-governance-contract \\
        --out <target/proxy-oracle-soroban/release-manifest.json>
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def sha256_file(path: Path) -> str:
    """Return the SHA-256 hex digest of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def run(args: list[str], cwd: str | None = None) -> str:
    """Run a command and return stripped stdout, or '' on failure."""
    try:
        result = subprocess.run(
            args,
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=30,
        )
        return result.stdout.strip()
    except Exception:
        return ""


def get_git_commit(root: Path) -> str:
    """Return the current HEAD commit hash (short), or 'unknown'."""
    out = run(["git", "rev-parse", "--short", "HEAD"], cwd=str(root))
    return out if out else "unknown"


def get_git_commit_full(root: Path) -> str:
    """Return the full HEAD commit hash, or 'unknown'."""
    out = run(["git", "rev-parse", "HEAD"], cwd=str(root))
    return out if out else "unknown"


def get_stellar_cli_version() -> str:
    """Return the stellar CLI version string, or 'unknown'."""
    out = run(["stellar", "--version"])
    if out:
        # First line: "stellar 25.1.0 (...)"
        return out.splitlines()[0].strip()
    return "unknown"


def get_rust_toolchain(root: Path) -> str:
    """Return the active Rust toolchain, or 'unknown'."""
    # Check rust-toolchain.toml first
    toolchain_file = root / "rust-toolchain.toml"
    if toolchain_file.exists():
        content = toolchain_file.read_text()
        m = re.search(r'channel\s*=\s*"([^"]+)"', content)
        if m:
            return m.group(1)
    # Fall back to `rustup show active-toolchain`
    out = run(["rustup", "show", "active-toolchain"])
    if out:
        return out.splitlines()[0].strip()
    # Fall back to `rustc --version`
    out = run(["rustc", "--version"])
    return out if out else "unknown"


def get_package_version(root: Path, pkg_name: str) -> str:
    """Parse the package version from cargo metadata."""
    out = run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=str(root),
    )
    if not out:
        return "unknown"
    try:
        meta = json.loads(out)
        for pkg in meta.get("packages", []):
            if pkg.get("name") == pkg_name:
                return pkg.get("version", "unknown")
    except Exception:
        pass
    return "unknown"


def build_deploy_command(
    wasm_path: str,
    network: str = "<network>",
    source: str = "<source-identity>",
) -> str:
    """Return the stellar contract install command (dry-run template, no broadcast)."""
    return (
        f"stellar contract install"
        f" --wasm {wasm_path}"
        f" --network {network}"
        f" --source {source}"
        f" --simulate-only"
    )


def build_initialize_command(
    contract_id: str = "<contract-id>",
    governance_id: str = "<governance-contract-id>",
    network: str = "<network>",
    source: str = "<source-identity>",
) -> str:
    """Return a template stellar contract invoke initialize command."""
    return (
        f"stellar contract invoke"
        f" --id {contract_id}"
        f" --network {network}"
        f" --source {source}"
        f" --simulate-only"
        f" -- initialize"
        f" --governance_id {governance_id}"
    )


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Write a deterministic release manifest for proxy-oracle Soroban WASMs."
    )
    parser.add_argument("--root", required=True, help="Workspace root directory")
    parser.add_argument("--runtime-wasm", required=True, help="Path to runtime optimized WASM")
    parser.add_argument("--governance-wasm", required=True, help="Path to governance optimized WASM")
    parser.add_argument(
        "--adapter-wasm",
        required=True,
        help="Path to SEP-40 adapter optimized WASM",
    )
    parser.add_argument(
        "--runtime-pkg",
        default="templar-proxy-oracle-soroban-contract",
        help="Runtime Cargo package name",
    )
    parser.add_argument(
        "--governance-pkg",
        default="templar-proxy-oracle-soroban-governance-contract",
        help="Governance Cargo package name",
    )
    parser.add_argument(
        "--adapter-pkg",
        default="templar-proxy-oracle-soroban-sep40-adapter-contract",
        help="SEP-40 adapter Cargo package name",
    )
    parser.add_argument("--out", required=True, help="Output path for release-manifest.json")
    args = parser.parse_args()

    root = Path(args.root).resolve()
    runtime_wasm = Path(args.runtime_wasm).resolve()
    governance_wasm = Path(args.governance_wasm).resolve()
    adapter_wasm = Path(args.adapter_wasm).resolve()
    out_path = Path(args.out).resolve()

    # Validate artifacts exist
    errors = []
    if not runtime_wasm.exists():
        errors.append(f"Runtime WASM not found: {runtime_wasm}")
    if not governance_wasm.exists():
        errors.append(f"Governance WASM not found: {governance_wasm}")
    if not adapter_wasm.exists():
        errors.append(f"SEP-40 adapter WASM not found: {adapter_wasm}")
    if errors:
        for e in errors:
            print(f"ERROR: {e}", file=sys.stderr)
        sys.exit(1)

    # Collect metadata
    git_commit = get_git_commit_full(root)
    git_commit_short = get_git_commit(root)
    stellar_cli = get_stellar_cli_version()
    rust_toolchain = get_rust_toolchain(root)
    runtime_version = get_package_version(root, args.runtime_pkg)
    governance_version = get_package_version(root, args.governance_pkg)
    adapter_version = get_package_version(root, args.adapter_pkg)

    runtime_sha256 = sha256_file(runtime_wasm)
    governance_sha256 = sha256_file(governance_wasm)
    adapter_sha256 = sha256_file(adapter_wasm)
    runtime_size = runtime_wasm.stat().st_size
    governance_size = governance_wasm.stat().st_size
    adapter_size = adapter_wasm.stat().st_size

    # Build dry-run command templates (relative paths for portability)
    def rel_or_abs(p: Path) -> str:
        try:
            return str(p.relative_to(root))
        except ValueError:
            return str(p)

    runtime_rel = rel_or_abs(runtime_wasm)
    governance_rel = rel_or_abs(governance_wasm)
    adapter_rel = rel_or_abs(adapter_wasm)

    deploy_cmd_runtime = build_deploy_command(runtime_rel)
    deploy_cmd_governance = build_deploy_command(governance_rel)
    deploy_cmd_adapter = build_deploy_command(adapter_rel)
    init_cmd_runtime = build_initialize_command()
    init_cmd_governance = build_initialize_command()

    manifest = {
        "schema_version": "2",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "git_commit": git_commit,
        "git_commit_short": git_commit_short,
        "stellar_cli": stellar_cli,
        "rust_toolchain": rust_toolchain,
        "runtime_wasm": {
            "package": args.runtime_pkg,
            "version": runtime_version,
            "path": runtime_rel,
            "sha256": runtime_sha256,
            "optimized_size": runtime_size,
        },
        "governance_wasm": {
            "package": args.governance_pkg,
            "version": governance_version,
            "path": governance_rel,
            "sha256": governance_sha256,
            "optimized_size": governance_size,
        },
        "sep40_adapter_wasm": {
            "package": args.adapter_pkg,
            "version": adapter_version,
            "path": adapter_rel,
            "sha256": adapter_sha256,
            "optimized_size": adapter_size,
        },
        "dry_run_commands": {
            "note": (
                "These commands use --simulate-only and do not broadcast. "
                "Replace <network>, <source-identity>, <contract-id>, "
                "<governance-contract-id>, <owner>, <parent-oracle-id>, "
                "<asset>, <decimals>, <resolution>, <base> with real values "
                "for actual deployment."
            ),
            "install_runtime": deploy_cmd_runtime,
            "install_governance": deploy_cmd_governance,
            "install_sep40_adapter": deploy_cmd_adapter,
            "initialize_runtime": init_cmd_runtime,
            "initialize_governance": init_cmd_governance,
        },
    }

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(manifest, indent=2) + "\n")

    print(f"Release manifest written: {out_path}")
    print(f"  git_commit:      {git_commit}")
    print(f"  stellar_cli:     {stellar_cli}")
    print(f"  rust_toolchain:  {rust_toolchain}")
    print(f"  runtime  sha256: {runtime_sha256}  ({runtime_size} bytes)")
    print(f"  governance sha256: {governance_sha256}  ({governance_size} bytes)")


if __name__ == "__main__":
    main()
