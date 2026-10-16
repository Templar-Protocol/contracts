#!/usr/bin/env python3
"""Offline schema-4 release validation and upload planning for Soroban.

No transaction or network simulation is performed and nothing is broadcast.
Printed upload templates require operator-selected NET and SRC values; consult
contract/proxy-oracle/soroban/RUNBOOK.md for constructor and governance steps.

Usage:
    python3 scripts/dry_run_deploy.py \\
        --manifest <target/proxy-oracle-soroban/release-manifest.json> \\
        --root <workspace_root> \\
        --out <evidence/task-8-dry-run.txt>
"""

import argparse
import json
import shlex
import sys
from pathlib import Path

from release_artifacts import (
    ARTIFACTS,
    atomic_write,
    parse_json,
    read_single_link_file,
    release_lock,
)
from validate_release_artifacts import validate_manifest, validation_report


def upload_plan(manifest: dict, root: Path) -> bytes:
    lines = [
        "",
        "Upload command templates (not executed):",
        "Set NET to the target network and SRC to the operator's signing identity.",
        "These commands broadcast when executed; no transaction or network "
        "simulation or broadcast was performed here.",
    ]
    for artifact in ARTIFACTS:
        path = root / manifest["artifacts"][artifact.slug]["path"]
        lines.append(
            f"{artifact.slug}: stellar contract upload "
            f'--network "$NET" --source "$SRC" --wasm {shlex.quote(str(path))}'
        )
    lines.append(
        "Constructor and governance steps: "
        "contract/proxy-oracle/soroban/RUNBOOK.md"
    )
    return ("\n".join(lines) + "\n").encode()


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Offline release artifact validation and upload planning."
    )
    parser.add_argument(
        "--manifest",
        required=True,
        help="Path to release-manifest.json",
    )
    parser.add_argument(
        "--root",
        required=True,
        help="Workspace root directory (used to resolve relative WASM paths)",
    )
    parser.add_argument(
        "--out",
        required=True,
        help="Output path for the dry-run evidence file",
    )
    args = parser.parse_args()

    manifest_path = Path(args.manifest).absolute()
    root = Path(args.root).resolve()
    out_path = Path(args.out).absolute()

    try:
        with release_lock():
            out_path.unlink(missing_ok=True)
            manifest_bytes = read_single_link_file(manifest_path, "release manifest")
            manifest = parse_json(manifest_bytes.decode())
            checked = validate_manifest(manifest, root)
            assert isinstance(manifest, dict)
            report = validation_report(manifest_bytes, manifest, checked)
            report += upload_plan(manifest, root)
            atomic_write(out_path, report)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1

    print(report.decode(), end="")
    print(f"Offline validation evidence written: {out_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
