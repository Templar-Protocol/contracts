#!/usr/bin/env python3
"""Build, optimize, size-check, or release proxy-oracle Soroban artifacts."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from release_artifacts import (
    ARTIFACTS,
    MANIFEST_PATH,
    RELEASE_DIR,
    ROOT,
    WASM_DIR,
    Artifact,
    atomic_write,
    fsync_directory,
    git_head,
    inspect_artifact_fd,
    invalidate_release_evidence,
    package_versions,
    read_single_link_file,
    release_lock,
    require_tracked_clean,
    run_text,
    selected_artifacts,
    stellar_version_metadata,
)


DEFAULT_TOOLCHAIN = "1.89.0"


def remap_rustflags(source_root: Path) -> str:
    return " ".join(
        (
            f"--remap-path-prefix {source_root}=/workspace",
            "--remap-path-prefix /nix/store=/nix-store",
        )
    )


def fail(message: str) -> None:
    raise ValueError(message)


def build_environment(toolchain: str, source_root: Path = ROOT) -> dict[str, str]:
    # The Stellar CLI drops CARGO_BUILD_RUSTFLAGS entirely when RUSTFLAGS is
    # set, and CARGO_ENCODED_RUSTFLAGS outranks both, so inherit none of them.
    dropped_rustflag_variables = {
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
    }
    environment = {
        name: value
        for name, value in os.environ.items()
        if name not in dropped_rustflag_variables
    }
    environment["RUSTUP_TOOLCHAIN"] = toolchain
    environment["CARGO_BUILD_RUSTFLAGS"] = remap_rustflags(source_root)
    return environment


def build_one(
    artifact: Artifact,
    output_directory: Path,
    toolchain: str,
    *,
    optimize: bool,
    source_root: Path = ROOT,
) -> Path:
    output_directory.mkdir(parents=True, exist_ok=True)
    command = [
        "stellar",
        "contract",
        "build",
        "--locked",
        "--profile",
        "release-soroban-checked",
        "--manifest-path",
        str(source_root / "Cargo.toml"),
        "--package",
        artifact.package,
        "--out-dir",
        str(output_directory),
    ]
    if optimize:
        command.append("--optimize")
    run_text(
        command,
        cwd=source_root,
        env=build_environment(toolchain, source_root),
    )
    output = output_directory / artifact.wasm
    payload = read_single_link_file(output, f"{artifact.slug} build output")
    if str(source_root).encode() in payload:
        fail(
            f"{artifact.slug}: Wasm embeds the host path {source_root}; "
            "the build is not reproducible"
        )
    return output


def inspect_built_artifact(path: Path, artifact: Artifact) -> dict[str, object]:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    descriptor = os.open(path, flags)
    try:
        measurement = inspect_artifact_fd(descriptor, artifact)
        return {
            "optimized_size": measurement.size,
            "sha256": measurement.wasm_sha256,
            "contract_spec_sha256": measurement.contract_spec_sha256,
        }
    finally:
        os.close(descriptor)


def publish_file(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as built:
        os.fsync(built.fileno())
    source.chmod(0o644)
    os.replace(source, destination)
    fsync_directory(destination.parent)


def developer_action(action: str, slug: str, toolchain: str) -> None:
    artifacts = selected_artifacts(slug)
    optimize = action in {"optimize", "size-check"}
    invalidate_release_evidence()
    with tempfile.TemporaryDirectory(prefix=".developer-", dir=RELEASE_DIR) as raw:
        staging = Path(raw)
        for artifact in artifacts:
            built = build_one(
                artifact,
                staging / ("optimized" if optimize else "unoptimized") / artifact.slug,
                toolchain,
                optimize=optimize,
            )
            if optimize:
                result = inspect_built_artifact(built, artifact)
                print(
                    f"{artifact.slug}: {result['optimized_size']} / "
                    f"{artifact.max_optimized_size} bytes"
                )
            if action != "size-check":
                destination = WASM_DIR / (
                    artifact.optimized_wasm if optimize else artifact.wasm
                )
                publish_file(built, destination)


def export_commit(commit: str, destination: Path) -> Path:
    """Lay out exactly the tracked content of `commit` and nothing else.

    A release attributes its digests to a commit, so it builds from one. An
    untracked, ignored, or half-saved file in the worktree cannot reach the
    bytes, which is a guarantee no amount of checking the worktree can give.
    """
    destination.mkdir(parents=True, exist_ok=True)
    archive = subprocess.run(
        ["git", "archive", "--format=tar", commit],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        check=False,
    )
    if archive.returncode:
        fail(f"git archive {commit} failed")
    extract = subprocess.run(
        ["tar", "-x", "-C", str(destination)],
        input=archive.stdout,
        check=False,
    )
    if extract.returncode:
        fail(f"extracting the {commit} archive failed")
    return destination


def release(toolchain: str) -> None:
    require_tracked_clean(ROOT)
    commit = git_head(ROOT)
    invalidate_release_evidence()
    cli_version, cli_output = stellar_version_metadata()
    rustc = run_text(["rustc", f"+{toolchain}", "--version"])
    if not rustc.startswith(f"rustc {toolchain}"):
        fail(f"rustc output does not match requested toolchain {toolchain}")
    staging = RELEASE_DIR / "release-build"
    if staging.exists():
        shutil.rmtree(staging)
    try:
        source_root = export_commit(commit, staging / "source")
        versions = package_versions(source_root)
        results: dict[str, dict[str, object]] = {}
        optimized_paths: dict[str, Path] = {}
        for artifact in ARTIFACTS:
            optimized = build_one(
                artifact,
                staging / "optimized" / artifact.slug,
                toolchain,
                optimize=True,
                source_root=source_root,
            )
            results[artifact.slug] = inspect_built_artifact(optimized, artifact)
            optimized_paths[artifact.slug] = optimized
        artifacts: dict[str, dict[str, object]] = {}
        for artifact in ARTIFACTS:
            result = results[artifact.slug]
            artifacts[artifact.slug] = {
                "package": artifact.package,
                "version": versions[artifact.package],
                "path": artifact.manifest_path,
                "sha256": result["sha256"],
                "contract_spec_sha256": result["contract_spec_sha256"],
                "optimized_size": result["optimized_size"],
                "max_optimized_size": artifact.max_optimized_size,
            }
        manifest = {
            "schema_version": "4",
            "generated_at": datetime.now(timezone.utc).strftime(
                "%Y-%m-%dT%H:%M:%SZ"
            ),
            "git_commit": commit,
            "stellar_cli": {"version": cli_version, "output": cli_output},
            "rust_toolchain": {"channel": toolchain, "rustc": rustc},
            "artifacts": artifacts,
        }
        invalidate_release_evidence()
        for artifact in ARTIFACTS:
            publish_file(
                optimized_paths[artifact.slug],
                WASM_DIR / artifact.optimized_wasm,
            )
        atomic_write(
            MANIFEST_PATH,
            (json.dumps(manifest, indent=2) + "\n").encode(),
        )
    finally:
        shutil.rmtree(staging, ignore_errors=True)


def execute(action: str, slug: str, toolchain: str) -> None:
    if not re.fullmatch(r"\d+\.\d+\.\d+", toolchain):
        fail("--rust-toolchain must be a numeric Rust channel")
    if action == "release":
        if slug != "all":
            fail("release requires --slug all")
        release(toolchain)
    else:
        developer_action(action, slug, toolchain)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--action",
        choices=("build", "optimize", "size-check", "release"),
        required=True,
    )
    parser.add_argument(
        "--slug",
        choices=("all", *(artifact.slug for artifact in ARTIFACTS)),
        default="all",
    )
    parser.add_argument("--rust-toolchain", default=DEFAULT_TOOLCHAIN)
    args = parser.parse_args()
    try:
        with release_lock():
            execute(args.action, args.slug, args.rust_toolchain)
    except (OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
