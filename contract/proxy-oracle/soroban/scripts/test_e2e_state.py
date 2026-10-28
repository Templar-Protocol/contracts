from __future__ import annotations

import base64
import copy
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

import e2e_state as rehearsal


def strkey(version: int, fill: int) -> str:
    body = bytes([version]) + bytes([fill]) * 32
    checksum = rehearsal.crc16_xmodem(body).to_bytes(2, "little")
    return base64.b32encode(body + checksum).decode().rstrip("=")


ACCOUNT = strkey(48, 7)
CONTRACT = strkey(16, 9)
OTHER_CONTRACT = strkey(16, 10)
DIGEST = "a" * 64
TX_HASH = "b" * 64


def settings(directory: Path) -> rehearsal.Settings:
    output = directory / "e2e"
    output.mkdir(mode=0o700)
    return rehearsal.Settings(
        output=output,
        snapshot=output / "snapshot",
        state_path=output / "state.json",
        rpc_url="https://rpc.test",
        horizon_url="https://horizon.test",
        source_identity="operator",
        administrator=ACCOUNT,
        asset_profile="xlm",
        asset={"Other": "XLM"},
        source_assets={
            "reflector": {"Other": "XLM"},
            "redstone": {"Stellar": CONTRACT},
            "lazer": {"Other": "23"},
        },
        feed_id=23,
        runtime_policy={
            "max_age_secs": 3600,
            "max_clock_drift_secs": 60,
        },
        ingest_policy={"max_age_secs": 300, "max_clock_drift_secs": 5},
        lazer_rest="https://lazer.test/latest",
    )


def checkpoint() -> dict[str, object]:
    context = {
        "network": {
            "name": rehearsal.NETWORK,
            "rpc_url": "https://rpc.test",
            "passphrase": rehearsal.NETWORK_IDENTIFIER,
        },
        "administrator": ACCOUNT,
        "git_commit": "f" * 40,
        "stellar_cli_version": "25.2.0",
        "tool_hashes": {
            path: "2" * 64 for path in rehearsal.TOOL_HASH_KEYS
        },
        "manifest_sha256": "1" * 64,
        "artifact_hashes": {
            slug: DIGEST for slug in rehearsal.SLUGS
        },
        "profile": {
            "name": "xlm",
            "asset": {"Other": "XLM"},
            "base": {"Other": "USD"},
            "reflector_asset": {"Other": "XLM"},
            "redstone_asset": {"Stellar": CONTRACT},
            "lazer_feed_id": 23,
        },
        "freshness": {
            "runtime": {
                "max_age_secs": 3600,
                "max_clock_drift_secs": 60,
            },
            "lazer_ingest": {
                "max_age_secs": 300,
                "max_clock_drift_secs": 5,
            },
        },
        "providers": {
            name: {
                "contract_id": CONTRACT,
                "initial_code_hash": "e" * 64,
            }
            for name in ("pyth_verifier", "reflector", "redstone")
        },
        "deployment_nonce": "d" * 64,
    }
    deployments = {
        slug: {
            "wasm_hash": DIGEST,
            "salt": "3" * 64,
            "contract_id": CONTRACT,
            "constructor_args": {},
            "verified": False,
        }
        for slug in rehearsal.SLUGS
    }
    return {
        "schema_version": rehearsal.SCHEMA_VERSION,
        "revision": 0,
        "context": context,
        "deployments": deployments,
        "operations": [],
        "phase_results": {
            phase: None for phase in rehearsal.PHASES
        },
    }


class FakeRunner:
    def __init__(
        self,
        *,
        fail_send: bool = False,
        send_raises: BaseException | None = None,
        decoded: str = '{"u32":1}',
    ):
        self.calls: list[tuple[list[str], str | None]] = []
        self.fail_send = fail_send
        self.send_raises = send_raises
        self.decoded = decoded

    def __call__(
        self, args: list[str], *, input_text: str | None = None
    ) -> subprocess.CompletedProcess[str]:
        self.calls.append((list(args), input_text))
        if args[:3] == ["stellar", "tx", "send"]:
            if self.send_raises is not None:
                raise self.send_raises
            if self.fail_send:
                raise rehearsal.RehearsalError("send transport failure")
        if args[:3] == ["stellar", "tx", "simulate"]:
            output = "simulated"
        elif args[:3] == ["stellar", "tx", "sign"]:
            output = "signed"
        elif args[:3] == ["stellar", "tx", "hash"]:
            output = TX_HASH
        elif args[:3] == ["stellar", "tx", "send"]:
            output = "submitted"
        elif args[:3] == ["stellar", "tx", "decode"]:
            output = json.dumps(
                {
                    "tx": {
                        "tx": {
                            "source_account": ACCOUNT,
                            "fee": 100,
                            "ext": "v0",
                        },
                        "signatures": (
                            [{"signature": "present"}]
                            if input_text == "signed"
                            else []
                        ),
                    }
                }
            )
        elif args[:3] == ["stellar", "xdr", "decode"]:
            output = self.decoded
        else:
            output = "unsigned"
        return subprocess.CompletedProcess(args, 0, output, "")


class RpcQueue:
    def __init__(self, *results: dict[str, object]):
        self.results = list(results)
        self.calls: list[tuple[str, str, object | None]] = []

    def __call__(self, url: str, method: str, params: object | None) -> object:
        self.calls.append((url, method, params))
        if not self.results:
            raise AssertionError("unexpected RPC call")
        return self.results.pop(0)


class CheckpointSchemaTests(unittest.TestCase):
    def test_old_aggregation_checkpoint_cannot_resume(self) -> None:
        value = checkpoint()
        value["schema_version"] = 1
        with self.assertRaisesRegex(rehearsal.RehearsalError, "unsupported checkpoint schema"):
            rehearsal.validate_checkpoint(value)

    def test_valid_checkpoint_accepts_reordered_phase_object(self) -> None:
        value = checkpoint()
        phase_results = value["phase_results"]
        assert isinstance(phase_results, dict)
        value["phase_results"] = dict(
            reversed(list(phase_results.items()))
        )
        self.assertIs(rehearsal.validate_checkpoint(value), value)

    def test_unknown_checkpoint_key_is_rejected(self) -> None:
        value = checkpoint()
        value["unexpected"] = True
        with self.assertRaisesRegex(rehearsal.RehearsalError, "exactly"):
            rehearsal.validate_checkpoint(value)

    def test_duplicate_json_key_and_nonfinite_number_are_rejected(self) -> None:
        for payload in (b'{"x":1,"x":2}', b'{"x":NaN}', b'{"x":1} trailing'):
            with self.subTest(payload=payload):
                with self.assertRaises(rehearsal.RehearsalError):
                    rehearsal.strict_json_bytes(payload, "fixture")

    def test_invalid_types_and_extra_context_are_rejected(self) -> None:
        cases = []
        bad_revision = checkpoint()
        bad_revision["revision"] = True
        cases.append(bad_revision)
        bad_context = checkpoint()
        context = bad_context["context"]
        assert isinstance(context, dict)
        context["extra"] = "drift"
        cases.append(bad_context)
        bad_feed = checkpoint()
        feed_context = bad_feed["context"]
        assert isinstance(feed_context, dict)
        profile = feed_context["profile"]
        assert isinstance(profile, dict)
        profile["lazer_feed_id"] = 1 << 32
        cases.append(bad_feed)
        for value in cases:
            with self.subTest(value=value):
                with self.assertRaises(rehearsal.RehearsalError):
                    rehearsal.validate_checkpoint(value)

    def test_two_unresolved_operations_are_rejected(self) -> None:
        value = checkpoint()
        operations = value["operations"]
        assert isinstance(operations, list)
        for number in (1, 2):
            operations.append(
                {
                    "number": number,
                    "phase": "refresh",
                    "kind": "refresh",
                    "target": CONTRACT,
                    "args": {"asset": {"Other": "XLM"}},
                    "status": "submitted",
                    "tx_hash": f"{number}" * 64,
                    "envelope_path": (
                        f"operations/{number:04d}/signed-envelope.xdr"
                    ),
                    "result_path": None,
                }
            )
        with self.assertRaisesRegex(rehearsal.RehearsalError, "more than one"):
            rehearsal.validate_checkpoint(value)

    def test_failed_operation_requires_result(self) -> None:
        value = checkpoint()
        operations = value["operations"]
        assert isinstance(operations, list)
        operations.append(
            {
                "number": 1,
                "phase": "refresh",
                "kind": "refresh",
                "target": CONTRACT,
                "args": {"asset": {"Other": "XLM"}},
                "status": "failed",
                "tx_hash": TX_HASH,
                "envelope_path": "operations/0001/signed-envelope.xdr",
                "result_path": None,
            }
        )
        with self.assertRaises(rehearsal.RehearsalError):
            rehearsal.validate_checkpoint(value)

    def test_corrupt_strkey_and_asset_shape_are_rejected(self) -> None:
        corrupted = ACCOUNT[:-1] + ("A" if ACCOUNT[-1] != "A" else "B")
        with self.assertRaisesRegex(rehearsal.RehearsalError, "checksum"):
            rehearsal.validate_account(corrupted, "account")
        with self.assertRaisesRegex(rehearsal.RehearsalError, "one-variant"):
            rehearsal.validate_asset({"Other": "XLM", "Stellar": CONTRACT}, "asset")
        self.assertEqual(rehearsal.validate_asset({"Other": "23"}, "asset"), {"Other": "23"})

    def test_freshness_shape_and_boolean_operation_number_are_rejected(
        self,
    ) -> None:
        missing_freshness_key = checkpoint()
        context = missing_freshness_key["context"]
        assert isinstance(context, dict)
        freshness = context["freshness"]
        assert isinstance(freshness, dict)
        freshness.pop("lazer_ingest")
        with self.assertRaisesRegex(rehearsal.RehearsalError, "exactly"):
            rehearsal.validate_checkpoint(missing_freshness_key)

        boolean_number = checkpoint()
        operations = boolean_number["operations"]
        assert isinstance(operations, list)
        operations.append(
            {
                "number": True,
                "phase": "refresh",
                "kind": "refresh",
                "target": CONTRACT,
                "args": {"asset": {"Other": "XLM"}},
                "status": "submitted",
                "tx_hash": TX_HASH,
                "envelope_path": "operations/0001/signed-envelope.xdr",
                "result_path": None,
            }
        )
        with self.assertRaisesRegex(rehearsal.RehearsalError, "u64"):
            rehearsal.validate_checkpoint(boolean_number)

    def test_checkpoint_uses_u64_revision_and_proposal_ids(self) -> None:
        value = checkpoint()
        value["revision"] = 1 << 64
        with self.assertRaisesRegex(
            rehearsal.RehearsalError, "u64"
        ):
            rehearsal.validate_checkpoint(value)

        value = checkpoint()
        operations = value["operations"]
        assert isinstance(operations, list)
        proposal_id = (1 << 32) + 1
        operations.append(
            {
                "number": 1,
                "phase": "configure",
                "kind": "proposal_create",
                "target": CONTRACT,
                "args": {
                    "caller": ACCOUNT,
                    "id": proposal_id,
                    "operation": {"SetProxy": []},
                    "requested_ttl": 0,
                },
                "status": "succeeded",
                "tx_hash": TX_HASH,
                "envelope_path": (
                    "operations/0001/signed-envelope.xdr"
                ),
                "result_path": "operations/0001/rpc-result.json",
            }
        )
        self.assertIs(rehearsal.validate_checkpoint(value), value)


class InputValidationTests(unittest.TestCase):
    def test_role_ttl_pages_reject_nonadvancing_or_malformed_offsets(self) -> None:
        self.assertIsNone(rehearsal.decode_role_ttl_offset("void", 32))
        self.assertEqual(rehearsal.decode_role_ttl_offset({"u32": 32}, 16), 32)
        for value in ({"u32": 16}, {"u32": 0}, {"u32": -1},
                      {"u32": 1 << 32}, {"u64": 32}, None):
            with self.subTest(value=value), self.assertRaises(rehearsal.RehearsalError):
                rehearsal.decode_role_ttl_offset(value, 16)

    def base_environment(self, directory: Path) -> dict[str, str]:
        return {"SRC": "operator", "OUT": str(directory / "out")}

    def test_xlm_profile_derives_lazer_asset_from_feed_id(self) -> None:
        with tempfile.TemporaryDirectory() as raw, mock.patch.dict(
            os.environ,
            self.base_environment(Path(raw)),
            clear=True,
        ), mock.patch.object(
            rehearsal, "resolve_administrator", return_value=ACCOUNT
        ):
            settings = rehearsal.load_settings()
        self.assertEqual(settings.feed_id, 23)
        self.assertEqual(
            settings.source_assets["lazer"],
            {"Other": str(settings.feed_id)},
        )

    def test_xlm_profile_rejects_field_override(self) -> None:
        with tempfile.TemporaryDirectory() as raw, mock.patch.dict(
            os.environ,
            {**self.base_environment(Path(raw)), "ASSET_SYMBOL": "XLM"},
            clear=True,
        ), mock.patch.object(rehearsal, "resolve_administrator", return_value=ACCOUNT):
            with self.assertRaisesRegex(rehearsal.RehearsalError, "forbids"):
                rehearsal.load_settings()

    def test_custom_profile_requires_every_field(self) -> None:
        with tempfile.TemporaryDirectory() as raw, mock.patch.dict(
            os.environ,
            {**self.base_environment(Path(raw)), "ASSET_PROFILE": "custom"},
            clear=True,
        ), mock.patch.object(rehearsal, "resolve_administrator", return_value=ACCOUNT):
            with self.assertRaisesRegex(rehearsal.RehearsalError, "requires"):
                rehearsal.load_settings()

    def test_custom_profile_rejects_duplicate_json_and_u32_overflow(self) -> None:
        environment = {
            "SRC": "operator",
            "ASSET_PROFILE": "custom",
            "ASSET_SYMBOL": "BTC",
            "REFLECTOR_ASSET_JSON": '{"Other":"BTC","Other":"ETH"}',
            "REDSTONE_ASSET_JSON": json.dumps({"Stellar": CONTRACT}),
            "LAZER_FEED_ID": str(1 << 32),
        }
        with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(
            rehearsal, "resolve_administrator", return_value=ACCOUNT
        ):
            with self.assertRaises(rehearsal.RehearsalError):
                rehearsal.load_settings()
        environment["REFLECTOR_ASSET_JSON"] = json.dumps({"Other": "BTC"})
        with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(
            rehearsal, "resolve_administrator", return_value=ACCOUNT
        ):
            with self.assertRaisesRegex(rehearsal.RehearsalError, "u32"):
                rehearsal.load_settings()

    def test_non_testnet_is_rejected_before_identity_resolution(self) -> None:
        with mock.patch.dict(os.environ, {"NET": "mainnet", "SRC": "operator"}, clear=True), mock.patch.object(
            rehearsal, "resolve_administrator"
        ) as resolve:
            with self.assertRaisesRegex(rehearsal.RehearsalError, "testnet"):
                rehearsal.load_settings()
            resolve.assert_not_called()

    def test_provider_override_is_rejected_before_identity_resolution(
        self,
    ) -> None:
        for name in ("PYTH_VERIFIER", "REFLECTOR", "REDSTONE"):
            with self.subTest(name=name), mock.patch.dict(
                os.environ,
                {"NET": "testnet", "SRC": "operator", name: CONTRACT},
                clear=True,
            ), mock.patch.object(
                rehearsal, "resolve_administrator"
            ) as resolve:
                with self.assertRaisesRegex(
                    rehearsal.RehearsalError, "provider contract IDs"
                ):
                    rehearsal.load_settings()
                resolve.assert_not_called()

    def test_lazer_rest_override_is_rejected_before_identity_resolution(
        self,
    ) -> None:
        with mock.patch.dict(
            os.environ,
            {
                "NET": "testnet",
                "SRC": "operator",
                "LAZER_REST": rehearsal.LAZER_REST,
            },
            clear=True,
        ), mock.patch.object(
            rehearsal, "resolve_administrator"
        ) as resolve:
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "LAZER_REST is fixed"
            ):
                rehearsal.load_settings()
            resolve.assert_not_called()

    def test_custom_profile_accepts_digit_prefixed_symbol(self) -> None:
        environment = {
            "SRC": "operator",
            "ASSET_PROFILE": "custom",
            "ASSET_SYMBOL": "1INCH",
            "REFLECTOR_ASSET_JSON": json.dumps({"Other": "1INCH"}),
            "REDSTONE_ASSET_JSON": json.dumps({"Stellar": CONTRACT}),
            "LAZER_FEED_ID": "23",
        }
        with mock.patch.dict(
            os.environ, environment, clear=True
        ), mock.patch.object(
            rehearsal, "resolve_administrator", return_value=ACCOUNT
        ):
            current = rehearsal.load_settings()
        self.assertEqual(current.asset, {"Other": "1INCH"})

    def test_feed_id_requires_canonical_decimal(self) -> None:
        environment = {
            "SRC": "operator",
            "ASSET_PROFILE": "custom",
            "ASSET_SYMBOL": "BTC",
            "REFLECTOR_ASSET_JSON": json.dumps({"Other": "BTC"}),
            "REDSTONE_ASSET_JSON": json.dumps({"Stellar": CONTRACT}),
            "LAZER_FEED_ID": "023",
        }
        with mock.patch.dict(
            os.environ, environment, clear=True
        ), mock.patch.object(
            rehearsal, "resolve_administrator", return_value=ACCOUNT
        ):
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "canonical"
            ):
                rehearsal.load_settings()

    def test_freshness_policy_enforces_contract_bounds(self) -> None:
        invalid = {
            "RUNTIME_MAX_AGE_SECS": "604801",
            "RUNTIME_MAX_CLOCK_DRIFT_SECS": "3601",
            "LAZER_INGEST_MAX_AGE_SECS": "604801",
            "LAZER_INGEST_MAX_CLOCK_DRIFT_SECS": "61",
        }
        for name, value in invalid.items():
            with self.subTest(name=name), mock.patch.dict(
                os.environ,
                {
                    **self.base_environment(Path("/tmp")),
                    name: value,
                },
                clear=True,
            ), mock.patch.object(
                rehearsal,
                "resolve_administrator",
                return_value=ACCOUNT,
            ):
                with self.assertRaises(rehearsal.RehearsalError):
                    rehearsal.load_settings()
    def test_secret_strkey_is_rejected_as_source_identity(self) -> None:
        environment = {"NET": "testnet", "SRC": "S" + "A" * 55}
        with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(
            rehearsal, "resolve_administrator"
        ) as resolve:
            with self.assertRaisesRegex(rehearsal.RehearsalError, "non-secret"):
                rehearsal.load_settings()
            resolve.assert_not_called()



class RpcTransportTests(unittest.TestCase):
    def test_rpc_call_sets_explicit_user_agent(self) -> None:
        class Response:
            def __enter__(self) -> Response:
                return self

            def __exit__(self, *args: object) -> None:
                return None

            def read(self) -> bytes:
                return rehearsal.canonical_json(
                    {
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {"passphrase": rehearsal.NETWORK_IDENTIFIER},
                    }
                )

        def urlopen(
            request: rehearsal.urllib.request.Request, *, timeout: int
        ) -> Response:
            self.assertEqual(timeout, 30)
            self.assertEqual(
                request.get_header("User-agent"),
                rehearsal.HTTP_USER_AGENT,
            )
            return Response()

        with mock.patch.object(
            rehearsal.urllib.request,
            "urlopen",
            side_effect=urlopen,
        ):
            result = rehearsal.rpc_call("https://rpc.test", "getNetwork")

        self.assertEqual(result, {"passphrase": rehearsal.NETWORK_IDENTIFIER})


class CheckpointStoreTests(unittest.TestCase):
    def make_store(self, directory: Path) -> rehearsal.CheckpointStore:
        store = rehearsal.CheckpointStore(directory / "state.json", checkpoint())
        store.first_save()
        return store

    def test_revision_conflict_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            first = self.make_store(directory)
            stale = rehearsal.CheckpointStore.load(directory / "state.json")
            first.mutate(lambda value: value.__setitem__("schema_version", rehearsal.SCHEMA_VERSION))
            with self.assertRaisesRegex(rehearsal.RehearsalError, "revision conflict"):
                stale.mutate(lambda value: value.__setitem__("schema_version", rehearsal.SCHEMA_VERSION))

    def test_phase_results_must_be_contiguous(self) -> None:
        value = checkpoint()
        phase_results = value["phase_results"]
        assert isinstance(phase_results, dict)
        phase_results["configure"] = {
            "operation_numbers": [],
            "evidence_path": "phases/configure.json",
        }
        with self.assertRaisesRegex(rehearsal.RehearsalError, "in order"):
            rehearsal.validate_checkpoint(value)

    def test_pass_phase_records_external_evidence_and_operation_numbers(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            store = self.make_store(directory)
            evidence = [{"verified_contracts": list(rehearsal.SLUGS)}]

            store.pass_phase("deploy", evidence)

            loaded = rehearsal.CheckpointStore.load(
                directory / "state.json"
            )
            phase_results = loaded.value["phase_results"]
            assert isinstance(phase_results, dict)
            self.assertEqual(
                phase_results["deploy"],
                {
                    "operation_numbers": [],
                    "evidence_path": "phases/deploy.json",
                },
            )
            self.assertEqual(
                json.loads((directory / "phases/deploy.json").read_text()),
                evidence,
            )

    def test_deterministic_salt_binds_context_and_slug(self) -> None:
        context = checkpoint()["context"]
        assert isinstance(context, dict)
        first = rehearsal.deterministic_salt(context, "runtime")
        self.assertEqual(
            first, rehearsal.deterministic_salt(context, "runtime")
        )
        changed = copy.deepcopy(context)
        changed["deployment_nonce"] = "b" * 64
        self.assertNotEqual(
            first, rehearsal.deterministic_salt(changed, "runtime")
        )
        self.assertNotEqual(
            first, rehearsal.deterministic_salt(context, "batcher")
        )

    def test_deployment_plan_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings = settings(Path(raw))
            context = checkpoint()["context"]
            assert isinstance(context, dict)
            with mock.patch.object(
                rehearsal,
                "deterministic_contract_id",
                return_value=CONTRACT,
            ):
                value = rehearsal.initialize_checkpoint(
                    current_settings, context
                )
                rehearsal.validate_deployment_plan(
                    current_settings, context, value["deployments"]
                )
                deployments = value["deployments"]
                assert isinstance(deployments, dict)
                runtime = deployments["runtime"]
                assert isinstance(runtime, dict)
                runtime["wasm_hash"] = "9" * 64
                with self.assertRaisesRegex(
                    rehearsal.RehearsalError, "deterministic plan"
                ):
                    rehearsal.validate_deployment_plan(
                        current_settings, context, deployments
                    )

    def test_resume_rejects_every_context_boundary_without_mutation(
        self,
    ) -> None:
        cases = (
            ("RPC", ("network", "rpc_url"), "https://changed-rpc.test"),
            ("administrator", ("administrator",), strkey(48, 8)),
            (
                "provider",
                ("providers", "reflector", "initial_code_hash"),
                "9" * 64,
            ),
            (
                "artifact",
                ("artifact_hashes", "runtime"),
                "8" * 64,
            ),
            (
                "tool",
                (
                    "tool_hashes",
                    "contract/proxy-oracle/soroban/scripts/e2e_state.py",
                ),
                "7" * 64,
            ),
            ("profile", ("profile", "name"), "custom"),
            (
                "policy",
                ("freshness", "runtime", "max_age_secs"),
                3601,
            ),
        )
        for label, path, changed_value in cases:
            with self.subTest(label=label), tempfile.TemporaryDirectory() as raw:
                current_settings = settings(Path(raw))
                existing = checkpoint()
                store = rehearsal.CheckpointStore(
                    current_settings.state_path, existing
                )
                store.first_save()
                before = current_settings.state_path.read_bytes()
                changed = copy.deepcopy(existing["context"])
                cursor = changed
                for key in path[:-1]:
                    assert isinstance(cursor, dict)
                    cursor = cursor[key]
                assert isinstance(cursor, dict)
                cursor[path[-1]] = changed_value

                with (
                    mock.patch.object(rehearsal, "check_network"),
                    mock.patch.object(rehearsal, "check_funded"),
                    mock.patch.object(
                        rehearsal,
                        "snapshot_release",
                        return_value=({}, {}),
                    ),
                    mock.patch.object(
                        rehearsal,
                        "provider_fingerprints",
                        return_value={},
                    ),
                    mock.patch.object(
                        rehearsal,
                        "build_context",
                        return_value=changed,
                    ),
                    mock.patch.object(
                        rehearsal, "validate_deployment_plan"
                    ) as validate_plan,
                    self.assertRaisesRegex(
                        rehearsal.RehearsalError,
                        "checkpoint context drift",
                    ),
                ):
                    rehearsal.resume_or_initialize(
                        current_settings, reinitialize=False
                    )

                validate_plan.assert_not_called()
                self.assertEqual(
                    current_settings.state_path.read_bytes(), before
                )


class ProviderDriftTests(unittest.TestCase):
    def test_drifted_provider_is_named_and_stops_the_resume(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings = settings(Path(raw))
            existing = checkpoint()
            store = rehearsal.CheckpointStore(
                current_settings.state_path, existing
            )
            store.first_save()
            stored_context = existing["context"]
            assert isinstance(stored_context, dict)
            stored_providers = stored_context["providers"]
            assert isinstance(stored_providers, dict)
            live = copy.deepcopy(stored_providers)
            live["redstone"]["initial_code_hash"] = "f" * 64
            drifted_context = copy.deepcopy(stored_context)
            drifted_context["providers"] = live

            with (
                mock.patch.object(rehearsal, "check_network"),
                mock.patch.object(rehearsal, "check_funded"),
                mock.patch.object(
                    rehearsal, "snapshot_release", return_value=({}, {})
                ),
                mock.patch.object(
                    rehearsal, "provider_fingerprints", return_value=live
                ),
                mock.patch.object(
                    rehearsal, "build_context", return_value=drifted_context
                ),
                self.assertRaisesRegex(
                    rehearsal.RehearsalError,
                    r"provider code drifted \(redstone\)",
                ),
            ):
                rehearsal.resume_or_initialize(
                    current_settings, reinitialize=False
                )


class TransactionStateMachineTests(unittest.TestCase):
    def make_store(self, directory: Path) -> rehearsal.CheckpointStore:
        store = rehearsal.CheckpointStore(directory / "e2e/state.json", checkpoint())
        store.first_save()
        return store

    def success_rpc(self, return_value: str = "AAAA") -> RpcQueue:
        return RpcQueue({"status": "SUCCESS", "returnValue": return_value})

    def test_exact_build_simulate_sign_hash_send_pipeline(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings = settings(directory)
            store = self.make_store(directory)
            runner = FakeRunner()
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=self.success_rpc(),
                sleep=lambda _: None,
            )
            operation = executor.execute(
                "refresh",
                "refresh",
                CONTRACT,
                {"asset": {"Other": "XLM"}},
                ["stellar", "contract", "invoke"],
            )
            self.assertEqual(operation["status"], "succeeded")
            self.assertEqual(
                [call[0][1:3] for call in runner.calls],
                [
                    ["contract", "invoke"],
                    ["tx", "simulate"],
                    ["tx", "sign"],
                    ["tx", "decode"],
                    ["tx", "decode"],
                    ["tx", "hash"],
                    ["tx", "decode"],
                    ["tx", "hash"],
                    ["tx", "send"],
                ],
            )
            self.assertEqual(runner.calls[1][1], "unsigned")
            self.assertEqual(runner.calls[2][1], "simulated")
            self.assertEqual(runner.calls[3][1], "simulated")
            for index in (4, 5, 6, 7, 8):
                self.assertEqual(runner.calls[index][1], "signed")
            loaded = rehearsal.CheckpointStore.load(current_settings.state_path)
            self.assertEqual(
                loaded.operations()[0]["status"], "succeeded"
            )

    def test_large_resource_fee_is_signed_and_recorded_as_fee_bump(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings = settings(directory)
            store = self.make_store(directory)
            calls: list[tuple[list[str], str | None]] = []
            encoded_payload: dict[str, object] = {}
            resource_fee = rehearsal.UINT32_MAX + 1

            def envelope(
                fee: int,
                extension: object,
                signatures: list[object],
            ) -> str:
                return json.dumps(
                    {
                        "tx": {
                            "tx": {
                                "source_account": ACCOUNT,
                                "fee": fee,
                                "ext": extension,
                            },
                            "signatures": signatures,
                        }
                    }
                )

            def runner(
                args: list[str], *, input_text: str | None = None
            ) -> subprocess.CompletedProcess[str]:
                calls.append((list(args), input_text))
                command = args[1:3]
                if command == ["tx", "simulate"]:
                    output = "simulated"
                elif command == ["tx", "decode"]:
                    if input_text == "simulated":
                        output = envelope(
                            0,
                            {"v1": {"resource_fee": str(resource_fee)}},
                            [],
                        )
                    elif input_text == "unsigned":
                        output = envelope(100, "v0", [])
                    elif input_text == "signed-inner":
                        output = envelope(
                            0,
                            {"v1": {"resource_fee": str(resource_fee)}},
                            [{"signature": "present"}],
                        )
                    elif input_text == "signed-fee-bump":
                        signed_fee_bump = copy.deepcopy(encoded_payload)
                        outer = signed_fee_bump["tx_fee_bump"]
                        assert isinstance(outer, dict)
                        outer["signatures"] = [{"signature": "outer"}]
                        output = json.dumps(signed_fee_bump)
                    else:
                        raise AssertionError(
                            f"unexpected envelope decode: {input_text!r}"
                        )
                elif command == ["tx", "encode"]:
                    assert input_text is not None
                    encoded_payload.update(json.loads(input_text))
                    output = "fee-bump"
                elif command == ["xdr", "encode"]:
                    output = base64.b64encode(
                        b"fee-bump-signature-payload"
                    ).decode()
                elif command == ["tx", "sign"]:
                    output = (
                        "signed-fee-bump"
                        if input_text == "fee-bump"
                        else "signed-inner"
                    )
                elif command == ["tx", "hash"]:
                    output = TX_HASH
                elif command == ["tx", "send"]:
                    output = "submitted"
                else:
                    output = "unsigned"
                return subprocess.CompletedProcess(args, 0, output, "")

            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=self.success_rpc(),
                sleep=lambda _: None,
            )
            executor.execute(
                "deploy",
                "deploy",
                CONTRACT,
                {
                    "slug": "runtime",
                    "wasm_hash": DIGEST,
                    "salt": "3" * 64,
                    "constructor_args": {},
                },
                ["stellar", "contract", "deploy"],
            )

            fee_bump = encoded_payload["tx_fee_bump"]
            assert isinstance(fee_bump, dict)
            transaction = fee_bump["tx"]
            assert isinstance(transaction, dict)
            self.assertEqual(
                transaction["fee"],
                str(resource_fee + 200),
            )
            self.assertEqual(transaction["fee_source"], ACCOUNT)
            self.assertEqual(
                transaction["inner_tx"],
                json.loads(
                    envelope(
                        0,
                        {"v1": {"resource_fee": str(resource_fee)}},
                        [{"signature": "present"}],
                    )
                ),
            )
            self.assertFalse(
                any(call[0][1:3] == ["tx", "hash"] for call in calls)
            )
            self.assertEqual(
                sum(call[0][1:3] == ["xdr", "encode"] for call in calls),
                2,
            )
            self.assertEqual(calls[-1][0][1:3], ["tx", "send"])
            self.assertEqual(calls[-1][1], "signed-fee-bump")
            persisted = (
                current_settings.output
                / "operations/0001/signed-envelope.xdr"
            ).read_text()
            self.assertEqual(persisted, "signed-fee-bump\n")

    def prepare_operation(
        self,
        directory: Path,
        *,
        submitted: bool,
        kind: str = "refresh",
        phase: str = "refresh",
        args: dict[str, object] | None = None,
    ) -> tuple[rehearsal.Settings, rehearsal.CheckpointStore]:
        current_settings = settings(directory)
        store = self.make_store(directory)
        envelope = current_settings.output / "operations/0001/signed-envelope.xdr"
        rehearsal.atomic_write(envelope, b"signed\n")
        operation = store.record_prepared(
            phase,
            kind,
            CONTRACT,
            args if args is not None else {"asset": {"Other": "XLM"}},
            TX_HASH,
            envelope,
        )
        if submitted:
            store.mark_submitted(operation)
        return current_settings, rehearsal.CheckpointStore.load(current_settings.state_path)

    def test_state_specific_transitions_validate_before_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings = settings(directory)
            store = self.make_store(directory)
            envelope = (
                current_settings.output
                / "operations/0001/signed-envelope.xdr"
            )
            rehearsal.atomic_write(envelope, b"signed\n")
            operation = store.record_prepared(
                "refresh",
                "refresh",
                CONTRACT,
                {"asset": {"Other": "XLM"}},
                TX_HASH,
                envelope,
            )
            result = (
                current_settings.output / "operations/0001/rpc-result.json"
            )
            before = copy.deepcopy(operation)
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "cannot complete"
            ):
                store.mark_succeeded(operation, result)
            self.assertEqual(operation, before)
            store.mark_submitted(operation)
            before = copy.deepcopy(operation)
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "requires an error"
            ):
                store.mark_failed(operation, result, "")
            self.assertEqual(operation, before)

    def test_resume_prepared_transaction_submits_same_envelope(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(Path(raw), submitted=False)
            runner = FakeRunner()
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=self.success_rpc(),
                sleep=lambda _: None,
            )
            executor.resolve_unfinished()
            self.assertEqual(runner.calls[0][0][1:3], ["tx", "decode"])
            self.assertEqual(runner.calls[0][1], "signed")
            self.assertEqual(runner.calls[1][0][1:3], ["tx", "hash"])
            self.assertEqual(runner.calls[1][1], "signed")
            self.assertEqual(runner.calls[2][0][1:3], ["tx", "send"])
            self.assertEqual(runner.calls[2][1], "signed")
            self.assertEqual(
                store.operations()[0]["status"], "succeeded"
            )

    def test_tampered_envelope_hash_is_rejected_before_submission(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw), submitted=False
            )
            runner = mock.Mock(
                side_effect=[
                    subprocess.CompletedProcess(
                        ["stellar", "tx", "decode"],
                        0,
                        json.dumps(
                            {
                                "tx": {
                                    "tx": {
                                        "source_account": ACCOUNT,
                                        "fee": 100,
                                        "ext": "v0",
                                    },
                                    "signatures": [
                                        {"signature": "present"}
                                    ],
                                }
                            }
                        ),
                        "",
                    ),
                    subprocess.CompletedProcess(
                        ["stellar", "tx", "hash"],
                        0,
                        "c" * 64,
                        "",
                    ),
                ]
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=self.success_rpc(),
                sleep=lambda _: None,
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "checkpoint hash"
            ):
                executor.resolve_unfinished()
            self.assertEqual(store.operations()[0]["status"], "prepared")
            self.assertEqual(runner.call_count, 2)

    def test_resume_submitted_transaction_only_polls_recorded_hash(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw), submitted=True
            )
            runner = mock.Mock(
                side_effect=AssertionError(
                    "submitted transaction must not invoke stellar CLI"
                )
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=self.success_rpc(),
                sleep=lambda _: None,
            )
            executor.resolve_unfinished()
            self.assertEqual(
                store.operations()[0]["status"], "succeeded"
            )
            runner.assert_not_called()
            self.assertFalse(
                (
                    current_settings.output
                    / "operations/0001/send.error"
                ).exists()
            )

    def test_pending_then_success_polls_without_duplicate_operation(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(Path(raw), submitted=True)
            rpc = RpcQueue(
                {"status": "NOT_FOUND"},
                {"status": "PENDING"},
                {"status": "SUCCESS", "returnValue": "AAAA"},
            )
            runner = mock.Mock(
                side_effect=AssertionError(
                    "submitted transaction must not invoke stellar CLI"
                )
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=rpc,
                sleep=lambda _: None,
            )
            executor.resolve_unfinished()
            self.assertEqual(len(store.operations()), 1)
            self.assertEqual(
                store.operations()[0]["status"], "succeeded"
            )
            runner.assert_not_called()

    def test_execute_reuses_a_state_bearing_operation_completed_on_resume(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw),
                submitted=False,
                kind="ownership_transfer",
                phase="ownership",
                args={"new_owner": CONTRACT, "live_until_ledger": 100},
            )
            runner = FakeRunner()
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=self.success_rpc(),
                sleep=lambda _: None,
            )
            operation = executor.execute(
                "ownership",
                "ownership_transfer",
                CONTRACT,
                {"new_owner": CONTRACT, "live_until_ledger": 100},
                ["stellar", "contract", "invoke", "must-not-build"],
            )
            self.assertEqual(operation["status"], "succeeded")
            self.assertEqual(len(store.operations()), 1)
            self.assertFalse(
                any("must-not-build" in call[0] for call in runner.calls)
            )

    def test_execute_resubmits_an_idempotent_maintenance_operation(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw), submitted=False
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=FakeRunner(),
                # One response resolves the recorded operation, one the resubmit.
                rpc=RpcQueue(
                    {"status": "SUCCESS", "returnValue": "AAAA"},
                    {"status": "SUCCESS", "returnValue": "AAAA"},
                ),
                sleep=lambda _: None,
            )
            operation = executor.execute(
                "refresh",
                "refresh",
                CONTRACT,
                {"asset": {"Other": "XLM"}},
                ["stellar", "contract", "invoke", "--id", CONTRACT],
            )
            self.assertEqual(operation["status"], "succeeded")
            self.assertEqual(operation["number"], 2)
            self.assertEqual(len(store.operations()), 2)

    def test_a_send_that_never_starts_is_recorded_as_a_failed_send(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw), submitted=False
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=FakeRunner(send_raises=OSError("cannot fork")),
                rpc=RpcQueue(),
                sleep=lambda _: None,
            )
            executor.submit(store.operations()[0])
            self.assertEqual(store.operations()[0]["status"], "send_failed")

    def test_repeated_resubmits_stay_resumable(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw), submitted=False
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=FakeRunner(),
                rpc=RpcQueue(
                    *(
                        {"status": "SUCCESS", "returnValue": "AAAA"}
                        for _ in range(4)
                    )
                ),
                sleep=lambda _: None,
            )
            args = {"asset": {"Other": "XLM"}}
            build = ["stellar", "contract", "invoke", "--id", CONTRACT]
            for expected_records in (2, 3):
                operation = executor.execute(
                    "refresh", "refresh", CONTRACT, args, build
                )
                self.assertEqual(operation["status"], "succeeded")
                self.assertEqual(len(store.operations()), expected_records)
            # The newest record stands for the step.
            self.assertEqual(
                store.terminal_operation("refresh", "refresh", CONTRACT, args)[
                    "number"
                ],
                3,
            )

    def test_a_failed_maintenance_operation_is_still_never_retried(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw), submitted=True
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=FakeRunner(),
                rpc=RpcQueue({"status": "FAILED", "resultXdr": "bad"}),
                sleep=lambda _: None,
            )
            with self.assertRaises(rehearsal.RehearsalError):
                executor.resolve_unfinished()
            self.assertEqual(store.operations()[0]["status"], "failed")
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "automatic retry is forbidden"
            ):
                executor.execute(
                    "refresh",
                    "refresh",
                    CONTRACT,
                    {"asset": {"Other": "XLM"}},
                    ["stellar", "contract", "invoke", "--id", CONTRACT],
                )

    def test_failed_rpc_result_is_terminal_and_persisted(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(Path(raw), submitted=True)
            runner = mock.Mock(
                side_effect=AssertionError(
                    "submitted transaction must not invoke stellar CLI"
                )
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=RpcQueue({"status": "FAILED", "errorResultXdr": "bad"}),
                sleep=lambda _: None,
            )
            with self.assertRaisesRegex(rehearsal.RehearsalError, "failed"):
                executor.resolve_unfinished()
            runner.assert_not_called()
            loaded = rehearsal.CheckpointStore.load(current_settings.state_path)
            operation = loaded.operations()[0]
            self.assertEqual(operation["status"], "failed")
            result_path = current_settings.output / str(
                operation["result_path"]
            )
            self.assertIn("errorResultXdr", result_path.read_text())

    def test_failed_operation_is_never_rebuilt_or_retried(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw), submitted=True
            )
            operation = store.operations()[0]
            result_path = (
                current_settings.output
                / "operations/0001/rpc-result.json"
            )
            rehearsal.atomic_write(
                result_path, b'{"status":"FAILED"}\n'
            )
            store.mark_failed(operation, result_path, "failed")
            runner = mock.Mock(
                side_effect=AssertionError(
                    "failed operation must not invoke stellar CLI"
                )
            )
            rpc = mock.Mock(
                side_effect=AssertionError(
                    "failed operation must not query transaction state"
                )
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=rpc,
                sleep=lambda _: None,
            )

            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "automatic retry is forbidden"
            ):
                executor.execute(
                    "refresh",
                    "refresh",
                    CONTRACT,
                    {"asset": {"Other": "XLM"}},
                    ["stellar", "contract", "invoke"],
                )

            runner.assert_not_called()
            rpc.assert_not_called()

    def test_submitted_timeout_never_resends_or_resigns(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            current_settings, store = self.prepare_operation(
                Path(raw), submitted=True
            )
            runner = mock.Mock(
                side_effect=AssertionError(
                    "submitted transaction must not invoke stellar CLI"
                )
            )
            rpc = RpcQueue(
                *({"status": "NOT_FOUND"} for _ in range(60))
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=rpc,
                sleep=lambda _: None,
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "remains unresolved"
            ):
                executor.resolve_unfinished()
            runner.assert_not_called()
            self.assertEqual(len(rpc.calls), 60)
            self.assertEqual(
                store.operations()[0]["status"], "submitted"
            )


    def test_ambiguous_send_stays_unresolved_until_network_finality(self) -> None:
        for status in ("NOT_FOUND", "PENDING"):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as raw:
                current_settings, store = self.prepare_operation(
                    Path(raw), submitted=False
                )
                executor = rehearsal.TransactionExecutor(
                    current_settings,
                    store,
                    runner=FakeRunner(fail_send=True),
                    rpc=RpcQueue(*({"status": status} for _ in range(60))),
                    sleep=lambda _: None,
                )
                with self.assertRaisesRegex(
                    rehearsal.RehearsalError, "remains unresolved"
                ):
                    executor.resolve_unfinished()
                loaded = rehearsal.CheckpointStore.load(current_settings.state_path)
                self.assertEqual(loaded.unresolved()["tx_hash"], TX_HASH)
                with self.assertRaisesRegex(
                    rehearsal.RehearsalError, "second unresolved"
                ):
                    loaded.record_prepared(
                        "ownership", "ownership_transfer", CONTRACT,
                        {"new_owner": OTHER_CONTRACT, "live_until_ledger": 100},
                        "c" * 64,
                        current_settings.output / "operations/0002/signed-envelope.xdr",
                    )
                resumed = rehearsal.TransactionExecutor(
                    current_settings,
                    loaded,
                    runner=mock.Mock(side_effect=AssertionError("must not resend")),
                    rpc=self.success_rpc(),
                    sleep=lambda _: None,
                )
                resumed.resolve_unfinished()
                self.assertEqual(
                    [(entry["tx_hash"], entry["status"]) for entry in loaded.operations()],
                    [(TX_HASH, "succeeded")],
                )
                self.assertIn(
                    "send transport failure",
                    (current_settings.output / "operations/0001/send.error").read_text(),
                )

    def test_zero_count_return_value_is_observable(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings, store = self.prepare_operation(directory, submitted=True)
            runner = FakeRunner(decoded='{"u32":0}')
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=self.success_rpc(),
                sleep=lambda _: None,
            )
            executor.resolve_unfinished()
            self.assertEqual(runner.calls, [])
            self.assertEqual(
                executor.return_value(store.operations()[0]), {"u32": 0}
            )

    def test_current_rpc_result_metadata_exposes_return_value(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings, store = self.prepare_operation(
                directory, submitted=True
            )
            runner = FakeRunner(
                decoded=json.dumps(
                    {
                        "v4": {
                            "ext": "v0",
                            "soroban_meta": {
                                "return_value": {"u32": 1}
                            },
                        }
                    }
                )
            )
            executor = rehearsal.TransactionExecutor(
                current_settings,
                store,
                runner=runner,
                rpc=RpcQueue(
                    {
                        "status": "SUCCESS",
                        "resultMetaXdr": "metadata-xdr",
                    }
                ),
                sleep=lambda _: None,
            )
            executor.resolve_unfinished()

            self.assertEqual(
                executor.return_value(store.operations()[0]), {"u32": 1}
            )
            self.assertEqual(
                runner.calls,
                [
                    (
                        [
                            "stellar",
                            "xdr",
                            "decode",
                            "--type",
                            "TransactionMeta",
                            "--input",
                            "single-base64",
                            "--output",
                            "json-formatted",
                        ],
                        "metadata-xdr",
                    )
                ],
            )

    def test_noncanonical_checkpoint_path_is_rejected_before_read(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings = settings(directory)
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                {
                    "number": 1,
                    "phase": "refresh",
                    "kind": "refresh",
                    "target": CONTRACT,
                    "args": {"asset": {"Other": "XLM"}},
                    "status": "submitted",
                    "tx_hash": TX_HASH,
                    "envelope_path": "../escape.xdr",
                    "result_path": None,
                }
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "noncanonical envelope"
            ):
                rehearsal.CheckpointStore(current_settings.state_path, value)


class RehearsalLogicTests(unittest.TestCase):
    class Transactions:
        def __init__(self, return_value: object | None = None):
            self.calls: list[
                tuple[
                    str, str, str, dict[str, object], list[str]
                ]
            ] = []
            self.decoded = return_value

        def execute(
            self,
            phase: str,
            kind: str,
            target: str,
            args: dict[str, object],
            command: list[str],
        ) -> dict[str, object]:
            self.calls.append((phase, kind, target, args, command))
            return {"status": "succeeded"}

        def return_value(self, _: object) -> object:
            return self.decoded

        def submitted_now(self, _: object) -> bool:
            return True

    def driver(
        self, directory: Path, value: dict[str, object] | None = None
    ) -> rehearsal.Rehearsal:
        current_settings = settings(directory)
        store = rehearsal.CheckpointStore(
            current_settings.state_path, value or checkpoint()
        )
        return rehearsal.Rehearsal(current_settings, store)

    def successful_operation(
        self,
        phase: str,
        kind: str,
        target: str,
        args: dict[str, object],
        *,
        number: int = 1,
    ) -> dict[str, object]:
        return {
            "number": number,
            "phase": phase,
            "kind": kind,
            "target": target,
            "args": args,
            "status": "succeeded",
            "tx_hash": TX_HASH,
            "envelope_path": (
                f"operations/{number:04d}/signed-envelope.xdr"
            ),
            "result_path": f"operations/{number:04d}/rpc-result.json",
        }

    def accepted_status(self) -> dict[str, object]:
        return {
            "vec": [
                {"symbol": "Accepted"},
                {
                    "map": [
                        {
                            "key": {"symbol": "expo"},
                            "val": {"i32": -8},
                        },
                        {
                            "key": {"symbol": "mantissa"},
                            "val": {"i64": "5000000000"},
                        },
                        {
                            "key": {"symbol": "timestamp"},
                            "val": {"u64": "123"},
                        },
                    ]
                },
            ]
        }

    def proposal(self, operation: object) -> dict[str, object]:
        return {
            "operation": operation,
            "created_at_ns": 1,
            "ttl_ns": 0,
            "created_by": ACCOUNT,
        }

    def test_deployment_postconditions_match_every_constructor(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            contract_ids = {
                slug: strkey(16, 20 + index)
                for index, slug in enumerate(rehearsal.SLUGS)
            }
            for slug, contract_id in contract_ids.items():
                deployment = driver.deployments[slug]
                deployment["contract_id"] = contract_id
                deployment["constructor_args"] = (
                    rehearsal.constructor_args(
                        driver.settings, contract_ids, slug
                    )
                )
            address_to_slug = {
                contract_id: slug
                for slug, contract_id in contract_ids.items()
            }

            def view(
                contract_id: str,
                function: str,
                arguments: dict[str, object] | None = None,
                *,
                decoder: object,
            ) -> object:
                del arguments, decoder
                slug = address_to_slug[contract_id]
                if function == "get_owner":
                    return ACCOUNT
                if slug == "runtime" and function == "source_base":
                    return {"Other": "USD"}
                if slug == "governance":
                    if function == "proxy_oracle":
                        return contract_ids["runtime"]
                    if function == "has_role":
                        return True
                    if function == "active_ids":
                        return []
                    if function == "get_operation_ttl":
                        return 0
                if slug == "lazer_source":
                    if function == "config":
                        return driver.deployments[slug][
                            "constructor_args"
                        ]["config"]  # type: ignore[index]
                    if function == "supported_feed_ids":
                        return [driver.settings.feed_id]
                    if function == "verification_epoch":
                        return 0
                    if function == "decimals":
                        return 8
                    if function == "resolution":
                        return 1
                if slug == "sep40_adapter" and function == "config":
                    constructor = driver.deployments[slug][
                        "constructor_args"
                    ]
                    assert isinstance(constructor, dict)
                    return {
                        key: value
                        for key, value in constructor.items()
                        if key != "owner"
                    }
                raise AssertionError(
                    f"unexpected view {slug}.{function}"
                )

            driver.view = view  # type: ignore[method-assign]
            for slug in rehearsal.DEPLOYMENT_ORDER:
                driver.verify_deployment_postconditions(slug)

    def test_preoccupied_runtime_owner_fails_constructor_check(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            runtime = driver.deployments["runtime"]
            runtime["constructor_args"] = {
                "governance": ACCOUNT,
                "base": {"Other": "USD"},
            }
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[{"Other": "USD"}, CONTRACT]
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "recorded lifecycle"
            ):
                driver.verify_deployment_postconditions("runtime")

    def test_deploy_uses_required_five_contract_order(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            transactions = self.Transactions()
            driver.transactions = transactions  # type: ignore[assignment]
            driver.verify_deployment = mock.Mock()  # type: ignore[method-assign]
            driver.store.pass_phase = mock.Mock()  # type: ignore[method-assign]

            driver.phase_deploy()

            self.assertEqual(
                [
                    (kind, arguments["slug"])
                    for _, kind, _, arguments, _ in transactions.calls
                ],
                [
                    *(
                        ("upload", slug)
                        for slug in rehearsal.DEPLOYMENT_ORDER
                    ),
                    *(
                        ("deploy", slug)
                        for slug in rehearsal.DEPLOYMENT_ORDER
                    ),
                ],
            )

    def test_deploy_build_uses_snapshotted_wasm_for_constructor_spec(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            driver.deployments["runtime"]["constructor_args"] = {
                "governance": ACCOUNT,
                "base": {"Other": "USD"},
            }

            command = driver.build_deploy("runtime")

            artifact = rehearsal.catalog_artifact("runtime")
            wasm_index = command.index("--wasm")
            self.assertEqual(
                command[wasm_index + 1],
                str(driver.settings.snapshot / artifact.optimized_wasm),
            )
            self.assertNotIn("--wasm-hash", command)
            constructor_index = command.index("--")
            self.assertEqual(
                command[constructor_index + 1 :],
                [
                    "--governance",
                    ACCOUNT,
                    "--base",
                    '{"Other":"USD"}',
                ],
            )

    def test_a_recorded_pending_proposal_is_not_unexpected_chain_state(
        self,
    ) -> None:
        """A crash between creating and executing a proposal must stay resumable.

        The governance postcondition runs on every invocation, so asserting no
        active proposals would abort before the phase that executes it.
        """
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                self.successful_operation(
                    "ownership",
                    "proposal_create",
                    CONTRACT,
                    {
                        "caller": ACCOUNT,
                        "id": 7,
                        "operation": "AcceptOwnership",
                        "requested_ttl": 0,
                    },
                )
            )
            driver = self.driver(Path(raw), value)
            self.assertEqual(
                driver.recorded_active_proposal_ids(CONTRACT), [7]
            )

            executed = self.successful_operation(
                "ownership",
                "proposal_execute",
                CONTRACT,
                {"caller": ACCOUNT, "id": 7},
                number=2,
            )
            operations.append(executed)
            self.assertEqual(driver.recorded_active_proposal_ids(CONTRACT), [])

    def governance_postcondition_driver(
        self, directory: Path, value: dict[str, object], active_ids: list[int]
    ) -> rehearsal.Rehearsal:
        driver = self.driver(directory, value)
        governance = str(driver.deployments["governance"]["contract_id"])
        views = {
            ("proxy_oracle", governance): str(
                driver.deployments["runtime"]["contract_id"]
            ),
            ("has_role", governance): True,
            ("active_ids", governance): active_ids,
        }
        driver.view = mock.Mock(  # type: ignore[method-assign]
            side_effect=lambda contract, method, *a, **k: views.get(
                (method, contract), 0
            )
        )
        return driver

    def test_an_unrecorded_active_proposal_is_rejected(self) -> None:
        """Subset, not equality: an unrecorded proposal is the unexpected one."""
        with tempfile.TemporaryDirectory() as raw:
            driver = self.governance_postcondition_driver(
                Path(raw), checkpoint(), [9]
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "unexpected active proposals"
            ):
                driver.verify_deployment_postconditions("governance")

    def test_a_recorded_active_proposal_passes_the_postcondition(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                self.successful_operation(
                    "ownership",
                    "proposal_create",
                    str(
                        value["deployments"]["governance"]["contract_id"]  # type: ignore[index]
                    ),
                    {
                        "caller": ACCOUNT,
                        "id": 9,
                        "operation": "AcceptOwnership",
                        "requested_ttl": 0,
                    },
                )
            )
            driver = self.governance_postcondition_driver(Path(raw), value, [9])
            driver.verify_deployment_postconditions("governance")

    def test_partial_timelock_hardening_requires_matching_execution_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            driver = self.governance_postcondition_driver(Path(raw), value, [])
            governance = str(driver.deployments["governance"]["contract_id"])
            operations = driver.store.value["operations"]
            assert isinstance(operations, list)
            ttls = {kind: 0 for kind in rehearsal.GOVERNANCE_OPERATION_KINDS}
            original_view = driver.view

            def view(contract, method, arguments=None, **kwargs):
                if method == "get_operation_ttl":
                    return ttls[arguments["kind"]]
                return original_view(contract, method, arguments, **kwargs)

            driver.view = view
            kind = "SetProxy"
            ttls[kind] = rehearsal.OPERATIONAL_TTL_NS
            create_args = {
                "caller": ACCOUNT, "id": 11, "requested_ttl": 0,
                "operation": {"SetActionTtl": [kind, rehearsal.OPERATIONAL_TTL_NS]},
            }
            operations.append(self.successful_operation(
                "configure", "proposal_create", governance, create_args,
            ))
            # A changed chain TTL is not evidence that an ambiguous execution
            # succeeded, nor may another proposal's receipt be adopted.
            for executed_id in (None, 12):
                if executed_id is not None:
                    operations.append(self.successful_operation(
                        "configure", "proposal_execute", governance,
                        {"caller": ACCOUNT, "id": executed_id}, number=2,
                    ))
                with self.assertRaises(rehearsal.RehearsalError):
                    driver.verify_deployment_postconditions("governance")
            operations.append(self.successful_operation(
                "configure", "proposal_execute", governance,
                {"caller": ACCOUNT, "id": 11}, number=3,
            ))
            driver.verify_deployment_postconditions("governance")
            ttls["SetActionTtl"] = rehearsal.OPERATIONAL_TTL_NS
            with self.assertRaises(rehearsal.RehearsalError):
                driver.verify_deployment_postconditions("governance")

    def test_confirmed_hardening_resume_does_not_create_replacement_proposals(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.governance_postcondition_driver(Path(raw), checkpoint(), [])
            governance = str(driver.deployments["governance"]["contract_id"])
            operations = driver.store.value["operations"]
            assert isinstance(operations, list)
            for index, kind in enumerate(rehearsal.GOVERNANCE_OPERATION_KINDS):
                operations.extend([
                    self.successful_operation(
                        "configure", "proposal_create", governance,
                        {
                            "caller": ACCOUNT, "id": index, "requested_ttl": 0,
                            "operation": {"SetActionTtl": [kind, rehearsal.OPERATIONAL_TTL_NS]},
                        }, number=index * 2 + 1,
                    ),
                    self.successful_operation(
                        "configure", "proposal_execute", governance,
                        {"caller": ACCOUNT, "id": index}, number=index * 2 + 2,
                    ),
                ])
            original_view = driver.view
            driver.view = lambda contract, method, arguments=None, **kwargs: (
                rehearsal.OPERATIONAL_TTL_NS if method == "get_operation_ttl"
                else original_view(contract, method, arguments, **kwargs)
            )
            transactions = self.Transactions()
            driver.transactions = transactions
            before = copy.deepcopy(driver.store.value)
            driver.verify_deployment_postconditions("governance")
            driver.harden_governance_timelocks()
            self.assertEqual(driver.store.value, before)
            self.assertEqual(transactions.calls, [])

    def test_pending_refresh_reuses_fresh_push_and_renews_stale_push(self) -> None:
        for phase in ("all", "refresh", "push"):
            for timestamp, failure in (
                (100, None),  # Inclusive freshness boundary.
                (99, None),
                (99, "build"),
                (99, "simulate"),
                (99, "sign"),
            ):
                with (
                    self.subTest(phase=phase, timestamp=timestamp, failure=failure),
                    tempfile.TemporaryDirectory() as raw,
                    mock.patch.object(rehearsal.time, "time", return_value=3700),
                ):
                    value = checkpoint()
                    value["operations"].append(self.successful_operation(
                        "push", "push", CONTRACT, {"payload": "44"}
                    ))
                    driver = self.driver(Path(raw), value)
                    driver.store.first_save()
                    for prior in ("deploy", "ownership", "configure"):
                        driver.store.pass_phase(prior, [])
                    rehearsal.atomic_write(
                        driver.settings.output / "operations/0001/rpc-result.json",
                        b'{"status":"SUCCESS","returnValue":"push"}',
                    )
                    recorded_response = (
                        driver.settings.output / "operations/0001/lazer_response.json"
                    )
                    rehearsal.atomic_write(
                        recorded_response,
                        json.dumps({
                            "request": driver.lazer_request_body(),
                            "response": {"leEcdsa": {"data": "44"}},
                        }).encode(),
                    )
                    original_response = recorded_response.read_bytes()
                    expected_timestamp = 3700 if timestamp == 99 else timestamp
                    accepted = self.accepted_status()
                    accepted["vec"][1]["map"][2]["val"]["u64"] = str(expected_timestamp)
                    decoded = {
                        "push": {"u32": 1},
                        "refresh": accepted,
                        "batch_refresh": {"vec": [accepted]},
                        "ttl_assets": {"vec": [{"bool": True}]},
                        "ttl_contracts": {"vec": [{"bool": True}] * 3},
                        **{f"ttl_role_{role}": "void" for role in rehearsal.GOVERNANCE_ROLES},
                    }
                    kinds = (
                        (["push"] if timestamp == 99 else [])
                        + ([] if phase == "push" else list(decoded)[1:])
                    )
                    queue = RpcQueue(*(
                        {"status": "SUCCESS", "returnValue": kind} for kind in kinds
                    ))
                    live = {"timestamp": timestamp}
                    cli = FakeRunner()
                    failing_command = {
                        "build": ["stellar", "contract", "invoke"],
                        "simulate": ["stellar", "tx", "simulate"],
                        "sign": ["stellar", "tx", "sign"],
                    }.get(failure)

                    def runner(args, *, input_text=None):
                        if args[:3] == failing_command:
                            raise rehearsal.RehearsalError("injected preparation failure")
                        if args[:3] == ["stellar", "xdr", "decode"]:
                            return subprocess.CompletedProcess(
                                args, 0, json.dumps(decoded[input_text]), ""
                            )
                        return cli(args, input_text=input_text)

                    def rpc(*args):
                        result = queue(*args)
                        if result.get("returnValue") == "push":
                            live["timestamp"] = 3700
                        return result

                    def view(_contract, function, _args=None, **_kwargs):
                        if function == "stored_price":
                            return {
                                "mantissa": 5_000_000_000, "expo": -8,
                                "publish_time_us": live["timestamp"] * 1_000_000,
                            }
                        if function == "aggregated_latest":
                            return {
                                "mantissa": 5_000_000_000, "expo": -8,
                                "timestamp": live["timestamp"],
                            }
                        return {"price": 5_000_000_000, "timestamp": live["timestamp"]}

                    driver.view = view
                    driver.transactions = rehearsal.TransactionExecutor(
                        driver.settings, driver.store, runner=runner, rpc=rpc,
                        sleep=lambda _: None,
                    )
                    response = mock.MagicMock()
                    response.__enter__.return_value = response
                    response.read.return_value = b'{"leEcdsa":{"data":"55"}}'
                    with (
                        mock.patch.object(
                            rehearsal.urllib.request, "urlopen", return_value=response
                        ),
                        mock.patch.object(driver, "pyth_api_key", return_value="token") as key,
                    ):
                        with mock.patch.object(
                            rehearsal.time, "time", return_value=timestamp
                        ):
                            driver.phase_push()
                        phase_evidence = driver.settings.output / "phases/push.json"
                        original_phase_evidence = phase_evidence.read_bytes()
                        renewal_payload = "55"
                        if failure is not None:
                            before = driver.settings.state_path.read_bytes()
                            with self.assertRaises(rehearsal.RehearsalError):
                                driver.run(phase)
                            self.assertEqual(
                                driver.settings.state_path.read_bytes(), before
                            )
                            self.assertEqual(
                                recorded_response.read_bytes(), original_response
                            )
                            driver.store = rehearsal.CheckpointStore.load(
                                driver.settings.state_path
                            )
                            driver.transactions.store = driver.store
                            failing_command = None
                            renewal_payload = "66"
                            response.read.return_value = b'{"leEcdsa":{"data":"66"}}'
                        if timestamp == 100:
                            key.side_effect = AssertionError("credential unavailable")
                        driver.run(phase)
                        operations = driver.store.operations()
                        self.assertEqual(
                            [entry["args"]["payload"] for entry in operations
                             if entry["kind"] == "push"],
                            ["44", renewal_payload] if timestamp == 99 else ["44"],
                        )
                        self.assertEqual(live["timestamp"], expected_timestamp)
                        self.assertEqual(
                            driver.store.value["phase_results"]["refresh"] is not None,
                            phase != "push",
                        )
                        self.assertEqual(
                            recorded_response.read_bytes(), original_response
                        )
                        self.assertEqual(
                            phase_evidence.read_bytes(), original_phase_evidence
                        )
                        before = driver.settings.state_path.read_bytes()
                        key.side_effect = AssertionError("credential unavailable")
                        driver.run(phase)
                        self.assertEqual(driver.settings.state_path.read_bytes(), before)

    def test_live_recorded_transfer_skips_duplicate_transfer(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                self.successful_operation(
                    "ownership",
                    "ownership_transfer",
                    CONTRACT,
                    {"new_owner": CONTRACT, "live_until_ledger": 200},
                )
            )
            driver = self.driver(Path(raw), value)
            transactions = self.Transactions()
            driver.transactions = transactions  # type: ignore[assignment]
            driver.latest_ledger = mock.Mock(return_value=100)  # type: ignore[method-assign]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[CONTRACT, ACCOUNT, CONTRACT]
            )
            driver.run_governance_operation = mock.Mock()  # type: ignore[method-assign]
            driver.store.pass_phase = mock.Mock()  # type: ignore[method-assign]

            driver.phase_ownership()

            self.assertEqual(transactions.calls, [])
            driver.run_governance_operation.assert_called_once()

    def test_absent_recorded_transfer_submits_transfer(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            transactions = self.Transactions()
            driver.transactions = transactions  # type: ignore[assignment]
            driver.latest_ledger = mock.Mock(return_value=100)  # type: ignore[method-assign]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[CONTRACT, ACCOUNT, CONTRACT]
            )
            driver.run_governance_operation = mock.Mock()  # type: ignore[method-assign]
            driver.store.pass_phase = mock.Mock()  # type: ignore[method-assign]

            driver.phase_ownership()

            self.assertEqual(len(transactions.calls), 1)
            self.assertEqual(
                transactions.calls[0][1:4],
                (
                    "ownership_transfer",
                    CONTRACT,
                    {
                        "new_owner": CONTRACT,
                        "live_until_ledger": 1_000_100,
                    },
                ),
            )

    def test_recorded_transfer_to_another_owner_stops_before_write(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                self.successful_operation(
                    "ownership",
                    "ownership_transfer",
                    CONTRACT,
                    {"new_owner": OTHER_CONTRACT, "live_until_ledger": 200},
                )
            )
            driver = self.driver(Path(raw), value)
            transactions = self.Transactions()
            driver.transactions = transactions  # type: ignore[assignment]
            driver.latest_ledger = mock.Mock(return_value=100)  # type: ignore[method-assign]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[CONTRACT, ACCOUNT]
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "targets another owner"
            ):
                driver.phase_ownership()
            self.assertEqual(transactions.calls, [])

    def test_expired_transfer_with_active_acceptance_stops(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.extend(
                [
                    self.successful_operation(
                        "ownership",
                        "ownership_transfer",
                        CONTRACT,
                        {
                            "new_owner": CONTRACT,
                            "live_until_ledger": 90,
                        },
                    ),
                    self.successful_operation(
                        "ownership",
                        "proposal_create",
                        CONTRACT,
                        {
                            "caller": ACCOUNT,
                            "id": 7,
                            "operation": "AcceptOwnership",
                            "requested_ttl": 0,
                        },
                        number=2,
                    ),
                ]
            )
            driver = self.driver(Path(raw), value)
            transactions = self.Transactions()
            driver.transactions = transactions  # type: ignore[assignment]
            driver.latest_ledger = mock.Mock(return_value=100)  # type: ignore[method-assign]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[
                    CONTRACT,
                    ACCOUNT,
                    self.proposal("AcceptOwnership"),
                    [7],
                ]
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "pending acceptance proposal"
            ):
                driver.phase_ownership()
            self.assertEqual(transactions.calls, [])

    def test_cancelled_recorded_proposal_is_not_recreated(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                self.successful_operation(
                    "configure",
                    "proposal_create",
                    CONTRACT,
                    {
                        "caller": ACCOUNT,
                        "id": 11,
                        "operation": {"SetProxy": []},
                        "requested_ttl": 0,
                    },
                )
            )
            driver = self.driver(Path(raw), value)
            transactions = self.Transactions()
            driver.transactions = transactions  # type: ignore[assignment]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[None, []]
            )

            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "consumed or cancelled"
            ):
                driver.run_governance_operation(
                    "configure",
                    "proxy/configure",
                    {"SetProxy": []},
                )

            self.assertEqual(transactions.calls, [])

    def test_active_proposal_requires_matching_next_id(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                self.successful_operation(
                    "configure",
                    "proposal_create",
                    CONTRACT,
                    {
                        "caller": ACCOUNT,
                        "id": 11,
                        "operation": {"SetProxy": []},
                        "requested_ttl": 0,
                    },
                )
            )
            driver = self.driver(Path(raw), value)
            driver.transactions = self.Transactions()  # type: ignore[assignment]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[
                    self.proposal({"SetProxy": []}),
                    [11],
                    99,
                ]
            )

            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "no longer matches"
            ):
                driver.run_governance_operation(
                    "configure",
                    "proxy/configure",
                    {"SetProxy": []},
                )

    def test_unit_governance_variant_is_passed_as_json_string(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            transactions = self.Transactions()
            driver.transactions = transactions  # type: ignore[assignment]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[
                    [],
                    7,
                    self.proposal("AcceptOwnership"),
                    8,
                    [7],
                    None,
                    [],
                ]
            )
            driver.run_governance_operation(
                "ownership",
                "runtime/accept-ownership",
                "AcceptOwnership",
            )
            create_command = transactions.calls[0][4]
            operation_index = create_command.index("--operation")
            self.assertEqual(
                create_command[operation_index + 1], '"AcceptOwnership"'
            )
            self.assertEqual(
                [call[1] for call in transactions.calls],
                ["proposal_create", "proposal_execute"],
            )
            self.assertEqual(
                [call[3]["id"] for call in transactions.calls],
                [7, 7],
            )

    def test_view_decodes_exact_json_at_the_command_boundary(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            valid = subprocess.CompletedProcess(
                ["stellar"], 0, stdout=json.dumps(CONTRACT), stderr=""
            )
            malformed = subprocess.CompletedProcess(
                ["stellar"], 0, stdout=f"{json.dumps(CONTRACT)} trailing", stderr=""
            )
            duplicate = subprocess.CompletedProcess(
                ["stellar"],
                0,
                stdout='{"price":1,"price":2,"timestamp":3}',
                stderr="",
            )
            with mock.patch.object(
                rehearsal,
                "run_simple",
                side_effect=[valid, malformed, duplicate],
            ):
                self.assertEqual(
                    driver.view(
                        CONTRACT,
                        "get_owner",
                        decoder=rehearsal.validate_contract,
                    ),
                    CONTRACT,
                )
                with self.assertRaises(rehearsal.RehearsalError):
                    driver.view(
                        CONTRACT,
                        "get_owner",
                        decoder=rehearsal.validate_contract,
                    )
                with self.assertRaises(rehearsal.RehearsalError):
                    driver.view(
                        CONTRACT,
                        "lastprice",
                        decoder=rehearsal.decode_optional_price,
                    )

    def test_sep40_price_normalizes_i128_json_strings(self) -> None:
        self.assertEqual(
            rehearsal.decode_optional_price(
                {"price": "21232754", "timestamp": "1790094000"},
                "price",
            ),
            {"price": 21232754, "timestamp": 1790094000},
        )

    def test_sep40_price_decodes_raw_simulation_result(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            built = subprocess.CompletedProcess(
                ["stellar"], 0, stdout="transaction-xdr\n", stderr=""
            )
            decoded = subprocess.CompletedProcess(
                ["stellar"],
                0,
                stdout=json.dumps(
                    {
                        "map": [
                            {
                                "key": {"symbol": "price"},
                                "val": {"i128": "21314030"},
                            },
                            {
                                "key": {"symbol": "timestamp"},
                                "val": {"u64": "1790089230"},
                            },
                        ]
                    }
                ),
                stderr="",
            )
            with (
                mock.patch.object(
                    rehearsal, "run_simple", side_effect=[built, decoded]
                ) as run,
                mock.patch.object(
                    rehearsal,
                    "rpc_call",
                    return_value={
                        "results": [{"auth": [], "xdr": "price-xdr"}]
                    },
                ) as rpc,
            ):
                self.assertEqual(
                    driver.sep40_price(CONTRACT, {"Other": "XLM"}),
                    {"price": 21314030, "timestamp": 1790089230},
                )

            self.assertIn("--build-only", run.call_args_list[0].args[0])
            rpc.assert_called_once_with(
                "https://rpc.test",
                "simulateTransaction",
                {"transaction": "transaction-xdr"},
            )
            self.assertEqual(
                run.call_args_list[1].kwargs["input_text"], "price-xdr"
            )

    def test_scval_price_decodes_void_as_none(self) -> None:
        self.assertIsNone(
            rehearsal.decode_scval_optional_price("void", "price")
        )


    def test_scval_price_rejects_duplicate_fields(self) -> None:
        encoded = {
            "map": [
                {
                    "key": {"symbol": "price"},
                    "val": {"i128": "1"},
                },
                {
                    "key": {"symbol": "price"},
                    "val": {"i128": "2"},
                },
                {
                    "key": {"symbol": "timestamp"},
                    "val": {"u64": "3"},
                },
            ]
        }
        with self.assertRaisesRegex(
            rehearsal.RehearsalError, "duplicate field"
        ):
            rehearsal.decode_scval_optional_price(encoded, "price")

    def test_governance_resume_reconciles_recorded_proposal_id(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            create_args = {
                "caller": ACCOUNT,
                "id": 11,
                "operation": {"SetProxy": []},
                "requested_ttl": 0,
            }
            operations.append(
                self.successful_operation(
                    "configure",
                    "proposal_create",
                    CONTRACT,
                    create_args,
                )
            )
            driver = self.driver(Path(raw), value)
            transactions = self.Transactions()
            driver.transactions = transactions  # type: ignore[assignment]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[
                    self.proposal({"SetProxy": []}),
                    [11],
                    12,
                    None,
                    [],
                ]
            )
            driver.run_governance_operation(
                "configure", "proxy/configure", {"SetProxy": []}
            )
            self.assertEqual(
                [call[1] for call in transactions.calls],
                ["proposal_execute"],
            )
            self.assertEqual(transactions.calls[0][3]["id"], 11)

    def test_configure_rejects_changed_pin_without_submitting_or_repinning(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            pins = {name: 8 for name in rehearsal.RUNTIME_SOURCE_NAMES}
            current = driver.proxy_config(pins)
            before = copy.deepcopy(driver.store.value)

            def view(_contract, function, _args=None, **_kwargs):
                if function == "base":
                    return {"Other": "USD"}
                if function == "decimals":
                    return 9
                if function == "get_proxy":
                    return current
                raise AssertionError(f"unexpected view {function}")

            driver.view = view
            driver.sep40_price = lambda *_: {"price": 1_000_000_000, "timestamp": 3700}
            with mock.patch.object(rehearsal.time, "time", return_value=3700):
                with self.assertRaisesRegex(rehearsal.RehearsalError, "conflicts"):
                    driver.phase_configure()
            self.assertEqual(driver.store.value, before)

    def test_recorded_push_requires_matching_saved_payload(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                self.successful_operation(
                    "push",
                    "push",
                    CONTRACT,
                    {"payload": "44"},
                )
            )
            driver = self.driver(Path(raw), value)
            rehearsal.atomic_write(
                driver.settings.output / "operations/0001/lazer_response.json",
                json.dumps(
                    {
                        "request": driver.lazer_request_body(),
                        "response": {"leEcdsa": {"data": "55"}},
                    }
                ).encode(),
            )
            driver.transactions = self.Transactions(  # type: ignore[assignment]
                {"u32": 1}
            )
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=AssertionError(
                    "mismatched evidence must stop before state views"
                )
            )

            with self.assertRaisesRegex(
                rehearsal.RehearsalError,
                "differs from the recorded transaction",
            ):
                driver.phase_push()

            driver.view.assert_not_called()

    def test_failed_push_is_never_retried_with_a_fresh_payload(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            failed = self.successful_operation(
                "push", "push", CONTRACT, {"payload": "44"}
            )
            failed["status"] = "failed"
            operations.append(failed)
            driver = self.driver(Path(raw), value)
            driver.fetch_lazer_payload = mock.Mock(  # type: ignore[method-assign]
                side_effect=AssertionError(
                    "a failed push must not fetch a new payload"
                )
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "automatic retry is forbidden"
            ):
                driver.phase_push()
            driver.fetch_lazer_payload.assert_not_called()

    def test_a_zero_count_push_is_rebuilt_not_reused(self) -> None:
        """A push that stored nothing accomplished nothing.

        The transaction succeeded, so it is not a failure to refuse — but the
        contract skipped every feed, so the step needs a fresh payload rather
        than a replay of the stale one.
        """
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            value = checkpoint()
            operations = value["operations"]
            assert isinstance(operations, list)
            operations.append(
                self.successful_operation(
                    "push",
                    "push",
                    CONTRACT,
                    {"payload": "44"},
                )
            )
            driver = self.driver(directory, value)
            rehearsal.atomic_write(
                driver.settings.output / "operations/0001/lazer_response.json",
                b'{"request":{},"response":{}}',
            )
            driver.transactions = self.Transactions(  # type: ignore[assignment]
                {"u32": 0}
            )
            driver.fetch_lazer_payload = mock.Mock(  # type: ignore[method-assign]
                return_value=("55", {"request": {}, "response": {}})
            )

            with self.assertRaisesRegex(rehearsal.RehearsalError, "zero"):
                driver.phase_push()

            # The recorded zero-feed push was discarded, not replayed.
            driver.fetch_lazer_payload.assert_called_once_with()
            phase_results = driver.store.value["phase_results"]
            assert isinstance(phase_results, dict)
            self.assertIsNone(phase_results["push"])

    def test_nonaccepted_refresh_cannot_reach_price_postconditions(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            driver.transactions = self.Transactions(  # type: ignore[assignment]
                {"vec": [{"symbol": "Blocked"}, {"u32": 2}]}
            )
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=AssertionError("views must not run")
            )
            with self.assertRaisesRegex(rehearsal.RehearsalError, "Accepted"):
                driver.phase_refresh()

    def test_refresh_accepted_value_must_equal_runtime_state(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            transactions = self.Transactions(self.accepted_status())
            driver.transactions = transactions  # type: ignore[assignment]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[
                    {
                        "mantissa": 6_000_000_000,
                        "expo": -8,
                        "timestamp": 123,
                    },
                    {"price": 6_000_000_000, "timestamp": 123},
                ]
            )
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "differs from aggregated_latest"
            ):
                driver.phase_refresh()
            self.assertEqual(len(transactions.calls), 1)

    def test_refresh_requires_exact_adapter_projection(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            transactions = self.Transactions(self.accepted_status())
            driver.transactions = transactions  # type: ignore[assignment]
            driver.view = mock.Mock(  # type: ignore[method-assign]
                side_effect=[
                    {
                        "mantissa": 5_000_000_000,
                        "expo": -8,
                        "timestamp": 123,
                    },
                    {"price": 5_000_000_001, "timestamp": 123},
                ]
            )
            with mock.patch.object(
                rehearsal.time, "time", return_value=123
            ), self.assertRaisesRegex(
                rehearsal.RehearsalError, "divergent"
            ):
                driver.phase_refresh()
            self.assertEqual(len(transactions.calls), 1)

    def test_later_phase_refuses_to_execute_before_prerequisites(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            driver = self.driver(Path(raw))
            configure = mock.Mock(
                side_effect=AssertionError("phase must not execute")
            )
            driver.phase_configure = configure  # type: ignore[method-assign]
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "preceding phase"
            ):
                driver.run("configure")
            configure.assert_not_called()

    def test_resume_rejects_deployed_code_drift_before_transactions(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            deployments = value["deployments"]
            assert isinstance(deployments, dict)
            runtime = deployments["runtime"]
            assert isinstance(runtime, dict)
            runtime["verified"] = True
            driver = self.driver(Path(raw), value)
            with mock.patch.object(
                rehearsal,
                "fetch_contract_hash",
                return_value="9" * 64,
            ), self.assertRaisesRegex(
                rehearsal.RehearsalError, "Wasm hash mismatch"
            ):
                driver.verify_recorded_deployments()

    def test_scval_postconditions_require_exact_shapes(self) -> None:
        accepted = self.accepted_status()
        rehearsal.require_accepted_status(accepted, "refresh")
        rehearsal.require_accepted_statuses(
            {"vec": [accepted]}, 1, "batch refresh"
        )
        rehearsal.require_true_scvals(
            {"vec": [{"bool": True}, {"bool": True}]}, 2, "batch"
        )
        invalid_accepted = (
            {"message": "Accepted"},
            {"vec": [{"symbol": "Accepted"}, {"map": []}]},
            {"vec": [{"symbol": "Blocked"}, {"u32": 2}]},
        )
        for result in invalid_accepted:
            with self.subTest(result=result), self.assertRaises(
                rehearsal.RehearsalError
            ):
                rehearsal.require_accepted_status(result, "refresh")
        invalid_bools = (
            {"vec": [{"bool": True}]},
            {"vec": [{"bool": True}, {"bool": True}, {"bool": True}]},
            {"vec": [{"bool": True}, {"bool": False}]},
            {"metadata": [{"bool": True}, {"bool": True}]},
        )
        for result in invalid_bools:
            with self.subTest(result=result), self.assertRaises(
                rehearsal.RehearsalError
            ):
                rehearsal.require_true_scvals(result, 2, "batch")

    def test_passed_phase_is_not_rerun(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            value = checkpoint()
            phase_results = value["phase_results"]
            assert isinstance(phase_results, dict)
            for phase in rehearsal.PHASES:
                phase_results[phase] = {
                    "operation_numbers": [],
                    "evidence_path": f"phases/{phase}.json",
                }
            driver = self.driver(Path(raw), value)
            for phase in rehearsal.PHASES:
                setattr(driver, f"phase_{phase}", mock.Mock())
            driver.run("all")
            for phase in rehearsal.PHASES:
                getattr(driver, f"phase_{phase}").assert_not_called()


class FilesystemAndSecretTests(unittest.TestCase):
    def test_snapshot_symlink_cannot_change_external_files_or_permissions(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings = settings(directory)
            outside = directory / "outside"
            outside.mkdir()
            manifest = outside / "release-manifest.json"
            manifest.write_bytes(b"keep")
            manifest.chmod(0o600)
            with rehearsal.output_lock(current_settings.output):
                current_settings.snapshot.symlink_to(outside, target_is_directory=True)
                with self.assertRaisesRegex(
                    rehearsal.RehearsalError, "snapshot directory must not be a symlink"
                ):
                    rehearsal.snapshot_release(current_settings)
            self.assertEqual(manifest.read_bytes(), b"keep")
            self.assertEqual(manifest.stat().st_mode & 0o777, 0o600)
            self.assertEqual(set(outside.iterdir()), {manifest})

    def test_secure_read_rejects_fifo_without_waiting_for_a_writer(self) -> None:
        scripts = Path(rehearsal.__file__).resolve().parent
        with tempfile.TemporaryDirectory() as raw:
            fifo = Path(raw) / "state.json"
            os.mkfifo(fifo)
            code = (
                f"import sys; sys.path.insert(0, {str(scripts)!r}); "
                "from pathlib import Path; from e2e_state import secure_read; "
                "secure_read(Path(sys.argv[1]), 'checkpoint')"
            )
            result = subprocess.run(
                [sys.executable, "-c", code, str(fifo)],
                capture_output=True, text=True, timeout=2,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("single-link regular file", result.stderr)

    def test_secure_read_rejects_symlink_and_multilink(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            original = directory / "original"
            original.write_bytes(b"content")
            symlink = directory / "symlink"
            symlink.symlink_to(original)
            with self.assertRaises(rehearsal.RehearsalError):
                rehearsal.secure_read(symlink, "symlink")
            linked = directory / "linked"
            os.link(original, linked)
            with self.assertRaises(rehearsal.RehearsalError):
                rehearsal.secure_read(original, "multilink")

    def test_unmarked_nonempty_output_refuses_destructive_reset(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "out"
            output.mkdir()
            sentinel = output / "keep"
            sentinel.write_text("do not delete")
            with self.assertRaisesRegex(
                rehearsal.RehearsalError, "non-empty unmarked"
            ):
                with rehearsal.output_lock(output):
                    rehearsal.clear_output(output)
            self.assertEqual(sentinel.read_text(), "do not delete")

    def test_marked_output_reset_preserves_lock_and_marker_only(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "out"
            with rehearsal.output_lock(output):
                (output / "state.json").write_text("{}")
                nested = output / "operations"
                nested.mkdir()
                (nested / "transcript").write_text("data")
                rehearsal.clear_output(output)
                self.assertEqual(
                    {child.name for child in output.iterdir()},
                    {".lock", rehearsal.OUTPUT_MARKER},
                )
                self.assertEqual(
                    (output / rehearsal.OUTPUT_MARKER).read_bytes(),
                    rehearsal.OUTPUT_MARKER_CONTENT,
                )

    def test_output_lock_serializes_processes(self) -> None:
        scripts = Path(rehearsal.__file__).resolve().parent
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "out"
            code = (
                "import sys,time; from pathlib import Path; "
                f"sys.path.insert(0,{str(scripts)!r}); "
                "from e2e_state import output_lock; "
                f"p=Path({str(output)!r}); "
                "\nwith output_lock(p):\n print('locked',flush=True); time.sleep(0.3)"
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
            with rehearsal.output_lock(output):
                pass
            elapsed = time.monotonic() - started
            self.assertEqual(process.wait(timeout=2), 0)
            process.stdout.close()
            self.assertGreaterEqual(elapsed, 0.2)

    def test_api_key_is_not_persisted_in_response_evidence(self) -> None:
        class Response:
            def __enter__(self) -> "Response":
                return self

            def __exit__(self, *_: object) -> None:
                return None

            def read(self) -> bytes:
                return b'{"leEcdsa":{"data":"aabb"}}'

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings = settings(directory)
            store = rehearsal.CheckpointStore(
                current_settings.state_path, checkpoint()
            )
            driver = rehearsal.Rehearsal(current_settings, store)
            key_file = directory / "pyth-api-key"
            key_file.write_text("super-secret\n")
            with mock.patch.dict(
                os.environ,
                {"PYTH_LAZER_API_KEY_FILE": str(key_file)},
                clear=False,
            ), mock.patch.object(
                rehearsal.urllib.request,
                "urlopen",
                return_value=Response(),
            ) as urlopen:
                payload, evidence = driver.fetch_lazer_payload()
            self.assertEqual(payload, "aabb")
            self.assertEqual(evidence["request"]["priceFeedIds"], [23])
            request = urlopen.call_args.args[0]
            self.assertEqual(
                request.get_header("User-agent"),
                rehearsal.HTTP_USER_AGENT,
            )
            persisted = (
                current_settings.output / "operations/0001/lazer_response.json"
            ).read_text()
            self.assertNotIn("super-secret", persisted)
            self.assertNotIn("Authorization", persisted)

    def test_api_key_requires_an_absolute_regular_file(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            current_settings = settings(directory)
            driver = rehearsal.Rehearsal(
                current_settings,
                rehearsal.CheckpointStore(
                    current_settings.state_path, checkpoint()
                ),
            )
            with mock.patch.dict(
                os.environ,
                {"PYTH_LAZER_API_KEY": "retired-secret"},
                clear=True,
            ), self.assertRaisesRegex(
                rehearsal.RehearsalError, "API_KEY_FILE is required"
            ):
                driver.pyth_api_key()
            with mock.patch.dict(
                os.environ,
                {"PYTH_LAZER_API_KEY_FILE": "relative-key"},
                clear=True,
            ), self.assertRaisesRegex(
                rehearsal.RehearsalError, "absolute path"
            ):
                driver.pyth_api_key()
            key_file = directory / "key"
            key_file.write_text("secret\n")
            symlink = directory / "key-link"
            symlink.symlink_to(key_file)
            with mock.patch.dict(
                os.environ,
                {"PYTH_LAZER_API_KEY_FILE": str(symlink)},
                clear=True,
            ), self.assertRaisesRegex(
                rehearsal.RehearsalError, "symlink"
            ):
                driver.pyth_api_key()

    def test_retired_direct_api_key_is_not_forwarded(self) -> None:
        secret = "retired-secret"
        completed = subprocess.CompletedProcess(
            ["stellar", "version"], 0, stdout="stellar 25.2.0\n", stderr=""
        )
        with mock.patch.dict(
            os.environ, {"PYTH_LAZER_API_KEY": secret}, clear=True
        ), mock.patch.object(
            rehearsal.subprocess, "run", return_value=completed
        ) as run:
            rehearsal.run_simple(["stellar", "version"])
        environment = run.call_args.kwargs["env"]
        self.assertNotIn("PYTH_LAZER_API_KEY", environment)
        self.assertNotIn(secret, environment.values())


if __name__ == "__main__":
    unittest.main()
