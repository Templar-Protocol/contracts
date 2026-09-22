#!/usr/bin/env python3
"""Validate a schema-4 proxy-oracle Soroban release manifest and its artifacts."""

from __future__ import annotations

import json
import re
import sys
from datetime import datetime
from pathlib import Path

from release_artifacts import (
    ARTIFACTS,
    EVIDENCE_PATH,
    MANIFEST_PATH,
    ROOT,
    SEMVER,
    SHA256,
    atomic_write,
    git_head,
    inspect_artifact_fd,
    open_artifact,
    package_versions,
    parse_json,
    read_single_link_file,
    release_lock,
    require_tracked_clean,
    sha256_bytes,
    stable_stellar_version,
)


def fail(message: str) -> None:
    raise ValueError(message)


def validate_timestamp(value: object) -> None:
    if not isinstance(value, str) or not re.fullmatch(
        r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", value
    ):
        fail("invalid generated_at timestamp")
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError as error:
        fail(f"invalid generated_at timestamp: {error}")
    if parsed.strftime("%Y-%m-%dT%H:%M:%SZ") != value:
        fail("generated_at timestamp is not canonical UTC")


def production_source_check(manifest: dict[str, object], trusted_root: Path) -> None:
    if trusted_root.resolve() != ROOT.resolve():
        return
    require_tracked_clean(ROOT)
    if manifest["git_commit"] != git_head(ROOT):
        fail("manifest commit does not match clean Git HEAD")


def validate_manifest(
    manifest: object, trusted_root: Path = ROOT
) -> dict[str, dict[str, object]]:
    if not isinstance(manifest, dict):
        fail("manifest must be an object")
    required = {
        "schema_version",
        "generated_at",
        "git_commit",
        "stellar_cli",
        "rust_toolchain",
        "artifacts",
    }
    if set(manifest) != required:
        fail("manifest keys do not exactly match schema-4")
    if manifest["schema_version"] != "4":
        fail("unsupported manifest schema")
    validate_timestamp(manifest["generated_at"])
    if not isinstance(manifest["git_commit"], str) or not re.fullmatch(
        r"[0-9a-f]{40}", manifest["git_commit"]
    ):
        fail("invalid manifest Git commit")
    cli = manifest["stellar_cli"]
    if (
        not isinstance(cli, dict)
        or set(cli) != {"version", "output"}
        or not isinstance(cli["version"], str)
        or not SEMVER.fullmatch(cli["version"])
        or not isinstance(cli["output"], str)
        or cli["output"] != cli["output"].strip()
        or stable_stellar_version(cli["output"]) != cli["version"]
    ):
        fail("invalid Stellar CLI metadata")
    toolchain = manifest["rust_toolchain"]
    if (
        not isinstance(toolchain, dict)
        or set(toolchain) != {"channel", "rustc"}
        or not isinstance(toolchain["channel"], str)
        or not re.fullmatch(r"\d+\.\d+\.\d+", toolchain["channel"])
        or not isinstance(toolchain["rustc"], str)
        or toolchain["rustc"] != toolchain["rustc"].strip()
        or not re.match(
            rf"^rustc {re.escape(toolchain['channel'])}(?:\s|$)",
            toolchain["rustc"],
        )
    ):
        fail("invalid Rust toolchain metadata")
    production_source_check(manifest, trusted_root)
    versions = package_versions(ROOT)
    artifacts = manifest["artifacts"]
    catalog = {artifact.slug: artifact for artifact in ARTIFACTS}
    if not isinstance(artifacts, dict) or set(artifacts) != set(catalog):
        fail("manifest artifact set does not match the catalog")
    checked: dict[str, dict[str, object]] = {}
    expected_keys = {
        "package",
        "version",
        "path",
        "sha256",
        "contract_spec_sha256",
        "optimized_size",
        "max_optimized_size",
    }
    for slug, artifact in catalog.items():
        entry = artifacts[slug]
        if not isinstance(entry, dict) or set(entry) != expected_keys:
            fail(f"{slug}: invalid artifact entry shape")
        if (
            entry["package"] != artifact.package
            or entry["version"] != versions[artifact.package]
            or entry["path"] != artifact.manifest_path
            or entry["max_optimized_size"] != artifact.max_optimized_size
        ):
            fail(f"{slug}: catalog or package metadata drift")
        if not isinstance(entry["version"], str) or not SEMVER.fullmatch(
            entry["version"]
        ):
            fail(f"{slug}: invalid package version")
        if not all(
            isinstance(entry[key], str) and SHA256.fullmatch(entry[key])
            for key in ("sha256", "contract_spec_sha256")
        ):
            fail(f"{slug}: invalid digest")
        size = entry["optimized_size"]
        if (
            type(size) is not int
            or size <= 0
            or size > artifact.max_optimized_size
        ):
            fail(f"{slug}: invalid optimized size")
        with open_artifact(trusted_root, entry["path"]) as (descriptor, _):
            measurement = inspect_artifact_fd(descriptor, artifact)
            if measurement.size != size:
                fail(f"{slug}: size mismatch")
            if measurement.wasm_sha256 != entry["sha256"]:
                fail(f"{slug}: Wasm digest mismatch")
            if (
                measurement.contract_spec_sha256
                != entry["contract_spec_sha256"]
            ):
                fail(f"{slug}: interface digest mismatch")
        checked[slug] = {
            "size": measurement.size,
            "sha256": measurement.wasm_sha256,
            "contract_spec_sha256": measurement.contract_spec_sha256,
        }
    return checked
def validation_report(
    manifest_bytes: bytes,
    manifest: dict[str, object],
    checked: dict[str, dict[str, object]],
) -> bytes:
    lines = [
        "artifact validation: PASS",
        "scope: artifact validation, not deployment simulation",
        f"manifest_sha256: {sha256_bytes(manifest_bytes)}",
        f"git_commit: {manifest['git_commit']}",
    ]
    for artifact in ARTIFACTS:
        result = checked[artifact.slug]
        lines.append(
            f"{artifact.slug}: size={result['size']} "
            f"sha256={result['sha256']} "
            f"contract_spec_sha256={result['contract_spec_sha256']}"
        )
    return ("\n".join(lines) + "\n").encode()

def main() -> int:
    with release_lock():
        EVIDENCE_PATH.unlink(missing_ok=True)
        try:
            manifest_bytes = read_single_link_file(
                MANIFEST_PATH, "release manifest"
            )
            manifest = parse_json(manifest_bytes.decode())
            checked = validate_manifest(manifest)
            assert isinstance(manifest, dict)
            atomic_write(
                EVIDENCE_PATH,
                validation_report(manifest_bytes, manifest, checked),
            )
        except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
            print(f"ERROR: {error}", file=sys.stderr)
            return 1
    print(f"artifact validation: PASS ({EVIDENCE_PATH})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
