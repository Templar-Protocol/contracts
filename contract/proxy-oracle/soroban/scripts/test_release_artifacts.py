from __future__ import annotations

import copy
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
import time
from pathlib import Path
from unittest import mock

import make_release_manifest as generator
import release_artifacts as catalog
import validate_release_artifacts as validator


SPEC_DIGEST = "ab" * 32
WASM_PREFIX = b"\x00asm"


def policy_entries(policy_name: str) -> list[dict[str, object]]:
    policy = catalog.ABI_POLICIES[policy_name]
    entries: list[dict[str, object]] = []
    for name, function in policy.functions.items():
        entries.append(
            {
                "function_v0": {
                    "name": name,
                    "inputs": copy.deepcopy(function["inputs"]),
                    "outputs": copy.deepcopy(function["outputs"]),
                }
            }
        )
    if policy.exact_function_names is not None:
        for name in policy.exact_function_names - policy.functions.keys():
            entries.append(
                {"function_v0": {"name": name, "inputs": [], "outputs": []}}
            )
    for name, fields in policy.structs.items():
        entries.append(
            {"udt_struct_v0": {"name": name, "fields": copy.deepcopy(fields)}}
        )
    return entries


class ManifestFixture:
    def __init__(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.versions = {
            artifact.package: "1.2.3-alpha.1+build.5"
            for artifact in catalog.ARTIFACTS
        }
        artifacts: dict[str, dict[str, object]] = {}
        for artifact in catalog.ARTIFACTS:
            path = self.root / artifact.manifest_path
            path.parent.mkdir(parents=True, exist_ok=True)
            payload = WASM_PREFIX + artifact.slug.encode()
            path.write_bytes(payload)
            artifacts[artifact.slug] = {
                "package": artifact.package,
                "version": self.versions[artifact.package],
                "path": artifact.manifest_path,
                "sha256": catalog.sha256_bytes(payload),
                "contract_spec_sha256": SPEC_DIGEST,
                "optimized_size": len(payload),
                "max_optimized_size": artifact.max_optimized_size,
            }
        self.manifest: dict[str, object] = {
            "schema_version": "4",
            "generated_at": "2026-09-22T04:06:00Z",
            "git_commit": "12" * 20,
            "stellar_cli": {
                "version": "25.2.0",
                "output": (
                    "stellar 25.2.0 (commit)\n"
                    "stellar-xdr 25.0.0 (commit)\n"
                    "xdr curr (commit)"
                ),
            },
            "rust_toolchain": {
                "channel": "1.89.0",
                "rustc": "rustc 1.89.0 (29483883e 2025-08-04)",
            },
            "artifacts": artifacts,
        }

    def close(self) -> None:
        self.temporary.cleanup()


class ReleaseManifestValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = ManifestFixture()
        self.version_patch = mock.patch.object(
            validator,
            "package_versions",
            return_value=self.fixture.versions,
        )
        self.interface_patch = mock.patch.object(
            catalog,
            "interface_from_fd",
            return_value=([], SPEC_DIGEST),
        )
        self.version_patch.start()
        self.interface_patch.start()

    def tearDown(self) -> None:
        self.interface_patch.stop()
        self.version_patch.stop()
        self.fixture.close()

    def validate(self, manifest: object | None = None) -> dict[str, dict[str, object]]:
        return validator.validate_manifest(
            self.fixture.manifest if manifest is None else manifest,
            self.fixture.root,
        )

    def test_valid_five_artifact_manifest_and_three_line_cli_output(self) -> None:
        checked = self.validate()
        self.assertEqual(set(checked), {artifact.slug for artifact in catalog.ARTIFACTS})

    def test_semver_parser_is_linear_and_preserves_legal_suffixes(self) -> None:
        for value in (
            "0.0.0",
            "1.2.3-alpha.1+build.5",
            "1.0.0-0.3.7",
            "1.0.0-x.7.z.92",
            "1.0.0+21AF26D3----117B344092BD",
        ):
            with self.subTest(value=value):
                self.assertTrue(catalog.is_semver(value))
        for value in (
            "",
            "1.2",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3-",
            "1.2.3+",
            "1.2.3-alpha..1",
            "1.2.3-01",
            "1.2.3-alpha_beta",
            "1.2.3+build+other",
            "0.0.0-0." + "--." * 1_000,
        ):
            with self.subTest(value=value):
                self.assertFalse(catalog.is_semver(value))



    def test_manifest_shape_metadata_and_numeric_rejections(self) -> None:
        mutations: list[tuple[str, object]] = [
            ("schema", {**self.fixture.manifest, "schema_version": "3"}),
            ("timestamp", {**self.fixture.manifest, "generated_at": "2026-02-30T00:00:00Z"}),
            ("commit", {**self.fixture.manifest, "git_commit": "A" * 40}),
            (
                "cli mismatch",
                {
                    **self.fixture.manifest,
                    "stellar_cli": {
                        "version": "25.3.0",
                        "output": self.fixture.manifest["stellar_cli"]["output"],
                    },
                },
            ),
        ]
        artifacts = self.fixture.manifest["artifacts"]
        for label, value in mutations:
            with self.subTest(label=label), self.assertRaises(ValueError):
                self.validate(value)
        for field, value in (
            ("optimized_size", True),
            ("optimized_size", -1),
            ("optimized_size", 0),
            ("sha256", "A" * 64),
            ("version", "01.2.3"),
        ):
            changed = copy.deepcopy(self.fixture.manifest)
            changed["artifacts"]["runtime"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                self.validate(changed)
        missing = copy.deepcopy(self.fixture.manifest)
        del missing["artifacts"]["batcher"]
        with self.assertRaises(ValueError):
            self.validate(missing)
        extra = copy.deepcopy(self.fixture.manifest)
        extra["artifacts"]["extra"] = copy.deepcopy(artifacts["runtime"])
        with self.assertRaises(ValueError):
            self.validate(extra)

    def test_duplicate_keys_nan_and_trailing_values_reject(self) -> None:
        for text in (
            '{"schema_version":"4","schema_version":"4"}',
            '{"value":NaN}',
            '{} {}',
        ):
            with self.subTest(text=text), self.assertRaises((ValueError, json.JSONDecodeError)):
                catalog.parse_json(text)

    def test_changed_bytes_and_spec_digest_reject(self) -> None:
        runtime = catalog.ARTIFACTS[0]
        path = self.fixture.root / runtime.manifest_path
        path.write_bytes(path.read_bytes() + b"x")
        with self.assertRaisesRegex(ValueError, "size mismatch"):
            self.validate()
        path.write_bytes(WASM_PREFIX + runtime.slug.encode())
        with mock.patch.object(
            catalog,
            "interface_from_fd",
            return_value=([], "cd" * 32),
        ), self.assertRaisesRegex(ValueError, "interface digest mismatch"):
            self.validate()

    def test_in_place_artifact_mutation_during_interface_read_rejects(self) -> None:
        runtime = catalog.ARTIFACTS[0]
        path = self.fixture.root / runtime.manifest_path

        def mutate_during_read(
            _descriptor: int, policy_name: str
        ) -> tuple[list[object], str]:
            if policy_name == runtime.spec_policy:
                payload = path.read_bytes()
                path.write_bytes(payload[:-1] + bytes([payload[-1] ^ 1]))
            return [], SPEC_DIGEST

        with mock.patch.object(
            catalog,
            "interface_from_fd",
            side_effect=mutate_during_read,
        ), self.assertRaisesRegex(ValueError, "changed during inspection"):
            self.validate()

    def test_symlinks_nonregular_files_and_multilinks_reject(self) -> None:
        runtime = catalog.ARTIFACTS[0]
        path = self.fixture.root / runtime.manifest_path
        outside = self.fixture.root / "outside.wasm"
        outside.write_bytes(path.read_bytes())
        path.unlink()
        path.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "nonsymlink"):
            self.validate()

        self.fixture.close()
        self.fixture = ManifestFixture()
        path = self.fixture.root / runtime.manifest_path
        sibling = path.with_name("linked.wasm")
        os.link(path, sibling)
        with self.assertRaisesRegex(ValueError, "single-link"):
            self.validate()

        self.fixture.close()
        self.fixture = ManifestFixture()
        ancestor = self.fixture.root / "target/proxy-oracle-soroban"
        external = self.fixture.root / "external"
        external.mkdir()
        for artifact in catalog.ARTIFACTS:
            source = self.fixture.root / artifact.manifest_path
            destination = external / Path(artifact.manifest_path).name
            destination.write_bytes(source.read_bytes())
        for child in (ancestor / "wasm").iterdir():
            child.unlink()
        (ancestor / "wasm").rmdir()
        (ancestor / "wasm").symlink_to(external, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "ancestor"):
            self.validate()

    def test_canonical_path_rejects_traversal_absolute_and_backslash(self) -> None:
        for path in ("../outside", "/tmp/outside", r"target\outside"):
            changed = copy.deepcopy(self.fixture.manifest)
            changed["artifacts"]["runtime"]["path"] = path
            with self.subTest(path=path), self.assertRaises(ValueError):
                self.validate(changed)


    def test_fifo_artifact_rejects_without_reading(self) -> None:
        runtime = catalog.ARTIFACTS[0]
        path = self.fixture.root / runtime.manifest_path
        path.unlink()
        os.mkfifo(path)
        with self.assertRaisesRegex(ValueError, "regular nonsymlink"):
            self.validate()

class AbiPolicyTests(unittest.TestCase):
    def test_reviewed_policies_accept_their_exact_required_surfaces(self) -> None:
        for artifact in catalog.ARTIFACTS:
            with self.subTest(slug=artifact.slug):
                catalog.validate_interface(
                    policy_entries(artifact.spec_policy), artifact.spec_policy
                )

        runtime = policy_entries("runtime_v1")
        runtime.append(
            {"function_v0": {"name": "unrelated", "inputs": [], "outputs": []}}
        )
        catalog.validate_interface(runtime, "runtime_v1")

        batcher = policy_entries("ownerless_batcher_v1")
        batcher.append(
            {"function_v0": {"name": "extra", "inputs": [], "outputs": []}}
        )
        with self.assertRaisesRegex(ValueError, "exactly"):
            catalog.validate_interface(batcher, "ownerless_batcher_v1")

    def test_missing_function_return_drift_and_forbidden_exports_reject(self) -> None:
        runtime = policy_entries("runtime_v1")
        runtime.pop(0)
        with self.assertRaises(ValueError):
            catalog.validate_interface(runtime, "runtime_v1")

        batcher = policy_entries("ownerless_batcher_v1")
        batcher[0]["function_v0"]["outputs"] = ["bool"]
        with self.assertRaises(ValueError):
            catalog.validate_interface(batcher, "ownerless_batcher_v1")

        adapter = policy_entries("sep40_adapter_v1")
        adapter.append(
            {"function_v0": {"name": "set_decimals", "inputs": [], "outputs": []}}
        )
        with self.assertRaisesRegex(ValueError, "forbidden"):
            catalog.validate_interface(adapter, "sep40_adapter_v1")

        ownerful_batcher = policy_entries("ownerless_batcher_v1")
        ownerful_batcher.append(
            {"function_v0": {"name": "get_owner", "inputs": [], "outputs": []}}
        )
        with self.assertRaises(ValueError):
            catalog.validate_interface(ownerful_batcher, "ownerless_batcher_v1")

    def test_duplicate_functions_and_malformed_interface_reject(self) -> None:
        entries = policy_entries("runtime_v1")
        entries.append(copy.deepcopy(entries[0]))
        with self.assertRaisesRegex(ValueError, "duplicate function"):
            catalog.validate_interface(entries, "runtime_v1")
        for value in ([], {}, ["not-an-object"]):
            with self.subTest(value=value), self.assertRaises(ValueError):
                catalog.validate_interface(value, "runtime_v1")


class OrchestrationTests(unittest.TestCase):
    def test_build_subprocess_failure_propagates(self) -> None:
        artifact = catalog.ARTIFACTS[0]
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            generator, "run_text", side_effect=ValueError("build failed")
        ):
            with self.assertRaisesRegex(ValueError, "build failed"):
                generator.build_one(
                    artifact, Path(raw), "1.89.0", optimize=True
                )

    def test_stellar_version_metadata_parses_one_cli_invocation(self) -> None:
        output = (
            "stellar 25.2.0 (commit)\n"
            "stellar-xdr 25.0.0 (commit)\n"
            "xdr curr (commit)"
        )
        with mock.patch.object(
            catalog, "run_text", return_value=output
        ) as run:
            self.assertEqual(
                catalog.stellar_version_metadata(),
                ("25.2.0", output),
            )
        run.assert_called_once_with(["stellar", "--version"])

    def test_size_check_builds_only_optimized_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as raw, mock.patch.object(
            generator, "RELEASE_DIR", Path(raw)
        ), mock.patch.object(
            generator, "invalidate_release_evidence"
        ), mock.patch.object(
            generator, "build_one", return_value=Path(raw) / "artifact.wasm"
        ) as build, mock.patch.object(
            generator,
            "inspect_built_artifact",
            return_value={"optimized_size": 1},
        ), redirect_stdout(io.StringIO()):
            generator.developer_action("size-check", "all", "1.89.0")

        self.assertEqual(build.call_count, len(catalog.ARTIFACTS))
        self.assertTrue(
            all(call.kwargs["optimize"] is True for call in build.call_args_list)
        )

    def test_release_rejects_dirty_source_before_build(self) -> None:
        with mock.patch.object(
            generator, "require_tracked_clean", side_effect=ValueError("dirty")
        ), mock.patch.object(generator, "build_one") as build:
            with self.assertRaisesRegex(ValueError, "dirty"):
                generator.release("1.89.0")
            build.assert_not_called()

    def test_release_build_failure_invalidates_prior_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            release_directory = Path(raw)
            manifest_path = release_directory / "release-manifest.json"
            evidence_path = (
                release_directory / "evidence/artifact-validation.txt"
            )
            evidence_path.parent.mkdir()
            manifest_path.write_text("{}")
            evidence_path.write_text("artifact validation: PASS\n")
            with mock.patch.object(
                generator, "RELEASE_DIR", release_directory
            ), mock.patch.object(
                catalog, "MANIFEST_PATH", manifest_path
            ), mock.patch.object(
                catalog, "EVIDENCE_PATH", evidence_path
            ), mock.patch.object(
                generator, "require_tracked_clean"
            ), mock.patch.object(
                generator, "git_head", return_value="12" * 20
            ), mock.patch.object(
                generator,
                "stellar_version_metadata",
                return_value=("25.2.0", "stellar 25.2.0"),
            ), mock.patch.object(
                generator,
                "run_text",
                return_value="rustc 1.89.0 (commit)",
            ), mock.patch.object(
                generator,
                "package_versions",
                return_value={
                    artifact.package: "1.0.0"
                    for artifact in catalog.ARTIFACTS
                },
            ), mock.patch.object(
                generator,
                "build_one",
                side_effect=ValueError("optimizer failed"),
            ):
                with self.assertRaisesRegex(ValueError, "optimizer failed"):
                    generator.release("1.89.0")
            self.assertFalse(manifest_path.exists())
            self.assertFalse(evidence_path.exists())

    def test_over_budget_output_fails_shared_inspection_path(self) -> None:
        artifact = catalog.Artifact("tiny", "tiny", 4, "runtime_v1")
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "tiny.wasm"
            path.write_bytes(WASM_PREFIX + b"x")
            with self.assertRaisesRegex(ValueError, "exceeds"):
                generator.inspect_built_artifact(path, artifact)

    def test_failed_validation_removes_stale_pass_report(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            manifest_path = directory / "release-manifest.json"
            evidence_path = directory / "artifact-validation.txt"
            manifest_path.write_text("{}")
            evidence_path.write_text("artifact validation: PASS\n")
            lock = mock.MagicMock()
            lock.__enter__.return_value = None
            lock.__exit__.return_value = False
            with mock.patch.object(validator, "MANIFEST_PATH", manifest_path), mock.patch.object(
                validator, "EVIDENCE_PATH", evidence_path
            ), mock.patch.object(validator, "release_lock", return_value=lock), mock.patch.object(
                validator, "validate_manifest", side_effect=ValueError("invalid")
            ), redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(validator.main(), 1)
            self.assertFalse(evidence_path.exists())

    def test_release_lock_serializes_readers_and_writers(self) -> None:
        scripts = Path(catalog.__file__).resolve().parent
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            code = (
                "import sys,time; from pathlib import Path; "
                f"sys.path.insert(0,{str(scripts)!r}); "
                "import release_artifacts as r; "
                f"r.RELEASE_DIR=Path({str(directory)!r}); "
                f"r.LOCK_PATH=Path({str(directory / '.release.lock')!r}); "
                "r.RELEASE_DIR.mkdir(parents=True,exist_ok=True); "
                "\nwith r.release_lock():\n print('locked',flush=True)\n time.sleep(0.35)"
            )
            process = subprocess.Popen(
                [sys.executable, "-c", code],
                stdout=subprocess.PIPE,
                text=True,
            )
            self.assertIsNotNone(process.stdout)
            assert process.stdout is not None
            self.assertEqual(process.stdout.readline().strip(), "locked")
            started = time.monotonic()
            with mock.patch.object(catalog, "RELEASE_DIR", directory), mock.patch.object(
                catalog, "LOCK_PATH", directory / ".release.lock"
            ):
                with catalog.release_lock():
                    pass
            elapsed = time.monotonic() - started
            self.assertEqual(process.wait(timeout=2), 0)
            process.stdout.close()
            self.assertGreaterEqual(elapsed, 0.25)

    def test_release_lock_rejects_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            target = directory / "outside"
            target.write_text("unchanged")
            lock_path = directory / ".release.lock"
            lock_path.symlink_to(target)
            with mock.patch.object(
                catalog, "RELEASE_DIR", directory
            ), mock.patch.object(
                catalog, "LOCK_PATH", lock_path
            ), self.assertRaises(OSError):
                with catalog.release_lock():
                    pass
            self.assertEqual(target.read_text(), "unchanged")

    def test_public_validator_main_uses_canonical_paths_without_arguments(self) -> None:
        self.assertEqual(
            validator.ROOT,
            Path(validator.__file__).resolve().parents[4],
        )
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            manifest_path = directory / "release-manifest.json"
            evidence_path = directory / "artifact-validation.txt"
            manifest_path.write_text("{}")
            lock = mock.MagicMock()
            lock.__enter__.return_value = None
            lock.__exit__.return_value = False
            with mock.patch.object(validator, "MANIFEST_PATH", manifest_path), mock.patch.object(
                validator, "EVIDENCE_PATH", evidence_path
            ), mock.patch.object(validator, "release_lock", return_value=lock), mock.patch.object(
                validator, "validate_manifest", return_value={}
            ), mock.patch.object(
                validator, "validation_report", return_value=b"artifact validation: PASS\n"
            ), redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(validator.main(), 0)
            self.assertEqual(
                evidence_path.read_text(), "artifact validation: PASS\n"
            )


if __name__ == "__main__":
    unittest.main()
