#!/usr/bin/env python3
"""Restart-safe, testnet-only rehearsal for the Soroban proxy-oracle stack."""

from __future__ import annotations

import argparse
import base64
import binascii
import contextlib
import fcntl
import json
import os
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterator, Sequence, TypeVar

from release_artifacts import (
    ARTIFACTS,
    MANIFEST_PATH,
    ROOT,
    SHA256,
    catalog_artifact,
    canonical_json,
    parse_json,
    release_lock,
    read_single_link_file,
    sha256_bytes,
    sha256_file,
    stable_stellar_version,
    stellar_version_metadata,
)
from validate_release_artifacts import validate_manifest

SCHEMA_VERSION = 1
NETWORK = "testnet"
PASSPHRASE = "Test SDF Network ; September 2015"
DEFAULT_RPC_URL = "https://soroban-testnet.stellar.org"
DEFAULT_HORIZON_URL = "https://horizon-testnet.stellar.org"
HTTP_USER_AGENT = "templar-proxy-oracle-rehearsal/1"
LAZER_REST = "https://pyth-lazer.dourolabs.app/v1/latest_price"
PYTH_VERIFIER = "CAYFT5JE3UQTKT4Q6ZOZK4FXVYVT6RE3MFC7STA4UB6WAEGBT65MRU52"
REFLECTOR = "CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63"
REDSTONE = "CA7MY6TYNL5Z5H5FYGMN7YWSY3JIZG7LFY3DZ26EEGRBQ2UKTFWHD4ZJ"
PHASES = ("deploy", "ownership", "configure", "push", "refresh")
DEPLOYMENT_ORDER = (
    "runtime",
    "governance",
    "lazer_source",
    "batcher",
    "sep40_adapter",
)
GOVERNANCE_OPERATION_KINDS = (
    "SetProxy",
    "RemoveProxy",
    "ConfigureBreakers",
    "AddBreaker",
    "RemoveBreaker",
    "Rearm",
    "SetEnforced",
    "SetManualTrip",
    "TransferOwnership",
    "AcceptOwnership",
    "RenounceOwnership",
    "SetActionTtl",
    "SetRole",
    "Upgrade",
)
OPERATION_KINDS = (
    "upload",
    "deploy",
    "ownership_transfer",
    "proposal_create",
    "proposal_execute",
    "push",
    "refresh",
    "batch_refresh",
    "ttl_assets",
    "ttl_contracts",
)
OPERATION_STATUSES = ("prepared", "submitted", "succeeded", "failed")
TERMINAL_STATUSES = ("succeeded", "failed")
SLUGS = tuple(artifact.slug for artifact in ARTIFACTS)
SYMBOL_RE = re.compile(r"^[A-Za-z0-9_]{1,32}$")
CONTRACT_RE = re.compile(r"^C[A-Z2-7]{55}$")
ACCOUNT_RE = re.compile(r"^G[A-Z2-7]{55}$")
IDENTITY_RE = re.compile(r"^[A-Za-z0-9._-]{1,64}$")
UINT32_MAX = (1 << 32) - 1
UINT64_MAX = (1 << 64) - 1
INT64_MAX = (1 << 63) - 1
MAX_FRESHNESS_AGE_SECS = 7 * 24 * 60 * 60
MAX_FRESHNESS_DRIFT_SECS = 60 * 60
PROVIDER_ID_OVERRIDE_KEYS = (
    "PYTH_VERIFIER",
    "PYTH_VERIFIER_ID",
    "PYTH_VERIFIER_CONTRACT_ID",
    "REFLECTOR",
    "REFLECTOR_ID",
    "REFLECTOR_CONTRACT_ID",
    "REDSTONE",
    "REDSTONE_ID",
    "REDSTONE_CONTRACT_ID",
)


@dataclass(frozen=True)
class FreshnessPolicyDescriptor:
    age_key: str
    drift_key: str
    age_environment: str
    drift_environment: str
    default_age: int
    default_drift: int


PROVIDER_FRESHNESS = {
    "reflector": FreshnessPolicyDescriptor(
        "reflector_max_age_secs",
        "reflector_max_clock_drift_secs",
        "REFLECTOR_MAX_AGE_SECS",
        "REFLECTOR_MAX_CLOCK_DRIFT_SECS",
        600,
        60,
    ),
    "redstone": FreshnessPolicyDescriptor(
        "redstone_max_age_secs",
        "redstone_max_clock_drift_secs",
        "REDSTONE_MAX_AGE_SECS",
        "REDSTONE_MAX_CLOCK_DRIFT_SECS",
        3_600,
        60,
    ),
    "lazer": FreshnessPolicyDescriptor(
        "lazer_max_age_secs",
        "lazer_max_clock_drift_secs",
        "LAZER_RUNTIME_MAX_AGE_SECS",
        "LAZER_RUNTIME_MAX_CLOCK_DRIFT_SECS",
        600,
        60,
    ),
}
CHECKPOINT_KEYS = {
    "schema_version",
    "revision",
    "context",
    "deployments",
    "operations",
    "phase_results",
}
CONTEXT_KEYS = {
    "network",
    "administrator",
    "git_commit",
    "stellar_cli_version",
    "tool_hashes",
    "manifest_sha256",
    "artifact_hashes",
    "profile",
    "freshness",
    "providers",
    "deployment_nonce",
}
NETWORK_KEYS = {"name", "rpc_url", "passphrase"}
PROFILE_KEYS = {
    "name",
    "asset",
    "base",
    "reflector_asset",
    "redstone_asset",
    "lazer_feed_id",
}
FRESHNESS_KEYS = {
    "reflector",
    "redstone",
    "lazer_runtime",
    "lazer_ingest",
    "max_cache_age_secs",
}
FRESHNESS_WINDOW_KEYS = {"max_age_secs", "max_clock_drift_secs"}
PROVIDER_KEYS = {"contract_id", "initial_code_hash"}
DEPLOYMENT_KEYS = {
    "wasm_hash",
    "salt",
    "contract_id",
    "constructor_args",
    "verified",
}
OPERATION_KEYS = {
    "number",
    "phase",
    "kind",
    "target",
    "args",
    "status",
    "tx_hash",
    "envelope_path",
    "result_path",
}
PHASE_RESULT_KEYS = {"operation_numbers", "evidence_path"}
OPERATION_PHASES = {
    "upload": "deploy",
    "deploy": "deploy",
    "ownership_transfer": "ownership",
    "proposal_create": ("ownership", "configure"),
    "proposal_execute": ("ownership", "configure"),
    "push": "push",
    "refresh": "refresh",
    "batch_refresh": "refresh",
    "ttl_assets": "refresh",
    "ttl_contracts": "refresh",
}
TOOL_HASH_KEYS = {
    "contract/proxy-oracle/soroban/scripts/e2e_live.sh",
    "contract/proxy-oracle/soroban/scripts/e2e_state.py",
    "contract/proxy-oracle/soroban/scripts/release_artifacts.py",
    "contract/proxy-oracle/soroban/scripts/validate_release_artifacts.py",
}
OUTPUT_MARKER = ".templar-proxy-oracle-e2e"
OUTPUT_MARKER_CONTENT = b"Templar proxy-oracle testnet rehearsal v1\n"
DEPLOYMENT_PLAN_KEYS = {
    "wasm_hash",
    "salt",
    "contract_id",
    "constructor_args",
}


class RehearsalError(RuntimeError):
    """A deterministic rehearsal validation or execution failure."""


def fail(message: str) -> None:
    raise RehearsalError(message)


def require_exact_keys(value: object, expected: set[str], label: str) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != expected:
        fail(f"{label} keys do not exactly match the schema")
    return value


def require_string(value: object, label: str, *, nonempty: bool = True) -> str:
    if not isinstance(value, str) or (nonempty and not value):
        fail(f"{label} must be a{' non-empty' if nonempty else ''} string")
    return value


def require_u32(value: object, label: str, *, positive: bool = False) -> int:
    if type(value) is not int or value < (1 if positive else 0) or value > UINT32_MAX:
        fail(f"{label} must be {'a positive ' if positive else 'an '}u32")
    return value

def require_u64(
    value: object, label: str, *, positive: bool = False
) -> int:
    if (
        type(value) is not int
        or value < (1 if positive else 0)
        or value > UINT64_MAX
    ):
        fail(f"{label} must be {'a positive ' if positive else 'an '}u64")
    return value


def require_integer(value: object, label: str) -> int:
    if type(value) is int:
        return value
    if isinstance(value, str) and re.fullmatch(r"-?(?:0|[1-9]\d*)", value):
        return int(value)
    fail(f"{label} must be an integer")

def crc16_xmodem(payload: bytes) -> int:
    crc = 0
    for byte in payload:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def validate_strkey(value: object, label: str, version_byte: int) -> str:
    text = require_string(value, label)
    pattern = CONTRACT_RE if version_byte == 16 else ACCOUNT_RE
    if not pattern.fullmatch(text):
        fail(f"{label} is not a canonical Stellar StrKey")
    try:
        decoded = base64.b32decode(text)
    except ValueError as error:
        fail(f"{label} is not valid base32: {error}")
    if len(decoded) != 35 or decoded[0] != version_byte:
        fail(f"{label} has the wrong StrKey version or length")
    expected = int.from_bytes(decoded[-2:], "little")
    if crc16_xmodem(decoded[:-2]) != expected:
        fail(f"{label} has an invalid StrKey checksum")
    return text


def validate_contract(value: object, label: str) -> str:
    return validate_strkey(value, label, 16)


def validate_account(value: object, label: str) -> str:
    return validate_strkey(value, label, 48)

def validate_address(value: object, label: str) -> str:
    text = require_string(value, label)
    if text.startswith("C"):
        return validate_contract(text, label)
    if text.startswith("G"):
        return validate_account(text, label)
    fail(f"{label} is not a contract or account address")

def require_boolean(value: object, label: str) -> bool:
    if type(value) is not bool:
        fail(f"{label} must be a boolean")
    return value


def decode_optional_asset(
    value: object, label: str
) -> dict[str, str] | None:
    if value is None:
        return None
    return validate_asset(value, label)


def decode_u32_list(value: object, label: str) -> list[int]:
    if not isinstance(value, list):
        fail(f"{label} must be an array")
    return [
        require_u32(entry, f"{label}[{index}]")
        for index, entry in enumerate(value)
    ]

def decode_u64_list(value: object, label: str) -> list[int]:
    if not isinstance(value, list):
        fail(f"{label} must be an array")
    return [
        require_u64(entry, f"{label}[{index}]")
        for index, entry in enumerate(value)
    ]


def decode_optional_pending_owner(
    value: object, label: str
) -> dict[str, object] | None:
    if value is None:
        return None
    pending = require_exact_keys(
        value, {"address", "live_until_ledger"}, label
    )
    validate_address(pending["address"], f"{label}.address")
    require_u32(
        pending["live_until_ledger"],
        f"{label}.live_until_ledger",
        positive=True,
    )
    return pending


def decode_optional_proposal(
    value: object, label: str
) -> dict[str, object] | None:
    if value is None:
        return None
    proposal = require_exact_keys(
        value,
        {"operation", "created_at_ns", "ttl_ns", "created_by"},
        label,
    )
    require_u64(
        proposal["created_at_ns"], f"{label}.created_at_ns"
    )
    require_u64(proposal["ttl_ns"], f"{label}.ttl_ns")
    validate_address(proposal["created_by"], f"{label}.created_by")
    return proposal


def decode_optional_lazer_config(
    value: object, label: str
) -> dict[str, object] | None:
    if value is None:
        return None
    config = require_exact_keys(
        value,
        {"verifier", "base", "decimals", "channel", "freshness"},
        label,
    )
    validate_contract(config["verifier"], f"{label}.verifier")
    validate_asset(config["base"], f"{label}.base")
    require_u32(config["decimals"], f"{label}.decimals")
    require_string(config["channel"], f"{label}.channel")
    freshness = require_exact_keys(
        config["freshness"],
        {"max_age_secs", "max_clock_drift_secs"},
        f"{label}.freshness",
    )
    require_u32(
        freshness["max_age_secs"],
        f"{label}.freshness.max_age_secs",
        positive=True,
    )
    require_u32(
        freshness["max_clock_drift_secs"],
        f"{label}.freshness.max_clock_drift_secs",
    )
    return config


def decode_optional_adapter_config(
    value: object, label: str
) -> dict[str, object] | None:
    if value is None:
        return None
    config = require_exact_keys(
        value,
        {
            "parent_oracle",
            "asset",
            "decimals",
            "resolution",
            "base",
        },
        label,
    )
    validate_contract(
        config["parent_oracle"], f"{label}.parent_oracle"
    )
    validate_asset(config["asset"], f"{label}.asset")
    require_u32(config["decimals"], f"{label}.decimals")
    require_u32(
        config["resolution"], f"{label}.resolution", positive=True
    )
    validate_asset(config["base"], f"{label}.base")
    return config


def validate_asset(value: object, label: str) -> dict[str, str]:
    if not isinstance(value, dict) or len(value) != 1:
        fail(f"{label} must be a one-variant asset object")
    variant, raw = next(iter(value.items()))
    if variant == "Other":
        symbol = require_string(raw, f"{label}.Other")
        if not SYMBOL_RE.fullmatch(symbol):
            fail(f"{label}.Other is not a valid Soroban symbol")
        return {variant: symbol}
    if variant == "Stellar":
        return {variant: validate_contract(raw, f"{label}.Stellar")}
    fail(f"{label} has unsupported variant {variant!r}")


def decode_nonnegative_integer(value: object, label: str) -> int:
    return require_u64(value, label)


def decode_optional_price(
    value: object, label: str
) -> dict[str, object] | None:
    if value is None:
        return None
    encoded = require_exact_keys(value, {"price", "timestamp"}, label)
    price = require_integer(encoded["price"], f"{label}.price")
    if not -(1 << 127) <= price < 1 << 127:
        fail(f"{label}.price must be an i128")
    timestamp = require_integer(
        encoded["timestamp"], f"{label}.timestamp"
    )
    require_u64(timestamp, f"{label}.timestamp")
    return {"price": price, "timestamp": timestamp}


def decode_scval_optional_price(
    value: object, label: str
) -> dict[str, object] | None:
    if value == "void":
        return None
    encoded = require_exact_keys(value, {"map"}, label)
    entries = encoded["map"]
    if not isinstance(entries, list):
        fail(f"{label}.map must be an array")
    fields: dict[str, int] = {}
    for index, raw_entry in enumerate(entries):
        entry_label = f"{label}.map[{index}]"
        entry = require_exact_keys(raw_entry, {"key", "val"}, entry_label)
        key = require_exact_keys(
            entry["key"], {"symbol"}, f"{entry_label}.key"
        )
        name = require_string(key["symbol"], f"{entry_label}.key.symbol")
        value_type = {"price": "i128", "timestamp": "u64"}.get(name)
        if value_type is None:
            fail(f"{label} contains an unexpected field {name!r}")
        if name in fields:
            fail(f"{label} contains duplicate field {name!r}")
        wrapped = require_exact_keys(
            entry["val"], {value_type}, f"{entry_label}.val"
        )
        fields[name] = require_integer(
            wrapped[value_type], f"{entry_label}.val.{value_type}"
        )
    if set(fields) != {"price", "timestamp"}:
        fail(f"{label} does not contain a complete SEP-40 price")
    price = fields["price"]
    if not -(1 << 127) <= price < 1 << 127:
        fail(f"{label}.price must be an i128")
    require_u64(fields["timestamp"], f"{label}.timestamp")
    return {"price": price, "timestamp": fields["timestamp"]}


def decode_optional_stored_price(
    value: object, label: str
) -> dict[str, object] | None:
    if value is None:
        return None
    price = require_exact_keys(
        value, {"mantissa", "expo", "publish_time_us"}, label
    )
    require_integer(price["mantissa"], f"{label}.mantissa")
    require_integer(price["expo"], f"{label}.expo")
    publish_time = require_integer(
        price["publish_time_us"], f"{label}.publish_time_us"
    )
    if publish_time < 0:
        fail(f"{label}.publish_time_us must be non-negative")
    return price


def decode_optional_normalized_price(
    value: object, label: str
) -> dict[str, object] | None:
    if value is None:
        return None
    price = require_exact_keys(
        value, {"mantissa", "expo", "timestamp"}, label
    )
    require_integer(price["mantissa"], f"{label}.mantissa")
    require_integer(price["expo"], f"{label}.expo")
    timestamp = require_integer(price["timestamp"], f"{label}.timestamp")
    if timestamp < 0:
        fail(f"{label}.timestamp must be non-negative")
    return price


def decode_optional_proxy_config(
    value: object, label: str
) -> dict[str, object] | None:
    if value is None:
        return None
    return require_exact_keys(
        value,
        {"sources", "min_sources", "max_cache_age_secs"},
        label,
    )


def require_normalized_price_scval(
    value: object, label: str
) -> dict[str, int]:
    payload = require_exact_keys(value, {"map"}, label)
    entries = payload["map"]
    if not isinstance(entries, list) or len(entries) != 3:
        fail(f"{label} must contain exactly three fields")
    fields: dict[str, object] = {}
    for index, raw_entry in enumerate(entries):
        entry = require_exact_keys(
            raw_entry, {"key", "val"}, f"{label}.map[{index}]"
        )
        key = require_exact_keys(
            entry["key"], {"symbol"}, f"{label}.map[{index}].key"
        )
        name = require_string(
            key["symbol"], f"{label}.map[{index}].key.symbol"
        )
        if name in fields:
            fail(f"{label} contains duplicate field {name}")
        fields[name] = entry["val"]
    if set(fields) != {"expo", "mantissa", "timestamp"}:
        fail(f"{label} has invalid normalized-price fields")
    expo = require_exact_keys(
        fields["expo"], {"i32"}, f"{label}.expo"
    )["i32"]
    if (
        type(expo) is not int
        or expo < -(1 << 31)
        or expo >= 1 << 31
    ):
        fail(f"{label}.expo.i32 must be an i32")
    mantissa_text = require_exact_keys(
        fields["mantissa"], {"i64"}, f"{label}.mantissa"
    )["i64"]
    if not isinstance(mantissa_text, str) or not re.fullmatch(
        r"-?(?:0|[1-9]\d*)", mantissa_text
    ):
        fail(f"{label}.mantissa.i64 must be a canonical integer string")
    mantissa = int(mantissa_text)
    if mantissa < -(1 << 63) or mantissa >= 1 << 63:
        fail(f"{label}.mantissa.i64 is out of range")
    timestamp_text = require_exact_keys(
        fields["timestamp"], {"u64"}, f"{label}.timestamp"
    )["u64"]
    if not isinstance(timestamp_text, str) or not re.fullmatch(
        r"0|[1-9]\d*", timestamp_text
    ):
        fail(f"{label}.timestamp.u64 must be a canonical integer string")
    timestamp = int(timestamp_text)
    if timestamp >= 1 << 64:
        fail(f"{label}.timestamp.u64 is out of range")
    return {
        "mantissa": mantissa,
        "expo": expo,
        "timestamp": timestamp,
    }


def require_accepted_status(
    value: object, label: str
) -> dict[str, int]:
    status = require_exact_keys(value, {"vec"}, label)["vec"]
    if (
        not isinstance(status, list)
        or len(status) != 2
        or status[0] != {"symbol": "Accepted"}
    ):
        fail(f"{label} did not return Accepted")
    return require_normalized_price_scval(
        status[1], f"{label}.Accepted"
    )


def require_accepted_statuses(
    value: object, expected_count: int, label: str
) -> list[dict[str, int]]:
    statuses = require_exact_keys(value, {"vec"}, label)["vec"]
    if not isinstance(statuses, list) or len(statuses) != expected_count:
        fail(
            f"{label} did not return exactly "
            f"{expected_count} accepted statuses"
        )
    return [
        require_accepted_status(status, f"{label}[{index}]")
        for index, status in enumerate(statuses)
    ]

def project_normalized_price(
    price: dict[str, int],
    *,
    decimals: int,
    resolution: int,
) -> dict[str, int]:
    if resolution <= 0:
        fail("adapter resolution must be positive")
    scale = decimals + price["expo"]
    if scale >= 0:
        if scale > 38:
            fail("normalized price cannot be represented by the adapter")
        projected = price["mantissa"] * 10**scale
    else:
        if -scale > 38:
            fail("normalized price cannot be represented by the adapter")
        divisor = 10 ** (-scale)
        magnitude = abs(price["mantissa"]) // divisor
        projected = (
            -magnitude if price["mantissa"] < 0 else magnitude
        )
    if projected < -(1 << 127) or projected >= 1 << 127:
        fail("normalized price cannot be represented by the adapter")
    if price["mantissa"] != 0 and projected == 0:
        fail("normalized price loses all precision in the adapter")
    return {
        "price": projected,
        "timestamp": price["timestamp"]
        - (price["timestamp"] % resolution),
    }


def require_true_scvals(
    value: object, expected_count: int, label: str
) -> None:
    values = require_exact_keys(value, {"vec"}, label)["vec"]
    if (
        not isinstance(values, list)
        or len(values) != expected_count
        or any(item != {"bool": True} for item in values)
    ):
        fail(
            f"{label} did not return exactly "
            f"{expected_count} true values"
        )


T = TypeVar("T")


def strict_json_bytes(payload: bytes, label: str) -> object:
    try:
        return parse_json(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        fail(f"invalid {label}: {error}")


def canonical_json_text(value: object) -> str:
    return canonical_json(value).decode()


def atomic_write(path: Path, payload: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor, raw = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(raw)
    try:
        os.fchmod(descriptor, mode)
        with os.fdopen(descriptor, "wb") as output:
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temporary.unlink(missing_ok=True)


def secure_read(path: Path, label: str) -> bytes:
    try:
        return read_single_link_file(path, label)
    except ValueError as error:
        fail(str(error))


def operation_directory(number: int) -> Path:
    return Path("operations") / f"{number:04d}"


def operation_envelope_path(number: int) -> Path:
    return operation_directory(number) / "signed-envelope.xdr"


def operation_result_path(number: int) -> Path:
    return operation_directory(number) / "rpc-result.json"

def phase_evidence_path(phase: str) -> Path:
    return Path("phases") / f"{phase}.json"


def validate_operation_args(
    kind: str, value: object, label: str
) -> dict[str, object]:
    args = value if isinstance(value, dict) else None
    if args is None:
        fail(f"{label} must be an object")
    if kind == "upload":
        args = require_exact_keys(args, {"slug"}, label)
        if args["slug"] not in SLUGS:
            fail(f"{label}.slug is invalid")
    elif kind == "deploy":
        args = require_exact_keys(
            args,
            {"slug", "wasm_hash", "salt", "constructor_args"},
            label,
        )
        if args["slug"] not in SLUGS:
            fail(f"{label}.slug is invalid")
        for key in ("wasm_hash", "salt"):
            if not isinstance(args[key], str) or not SHA256.fullmatch(args[key]):
                fail(f"{label}.{key} is invalid")
        if not isinstance(args["constructor_args"], dict):
            fail(f"{label}.constructor_args must be an object")
    elif kind == "ownership_transfer":
        args = require_exact_keys(
            args, {"new_owner", "live_until_ledger"}, label
        )
        validate_contract(args["new_owner"], f"{label}.new_owner")
        require_u32(
            args["live_until_ledger"],
            f"{label}.live_until_ledger",
            positive=True,
        )
    elif kind == "proposal_create":
        args = require_exact_keys(
            args, {"caller", "id", "operation", "requested_ttl"}, label
        )
        validate_account(args["caller"], f"{label}.caller")
        require_u64(args["id"], f"{label}.id")
        require_u64(args["requested_ttl"], f"{label}.requested_ttl")
        if not isinstance(args["operation"], (dict, str)):
            fail(f"{label}.operation is invalid")
    elif kind == "proposal_execute":
        args = require_exact_keys(args, {"caller", "id"}, label)
        validate_account(args["caller"], f"{label}.caller")
        require_u64(args["id"], f"{label}.id")
    elif kind == "push":
        args = require_exact_keys(args, {"payload"}, label)
        payload = args["payload"]
        if (
            not isinstance(payload, str)
            or not payload
            or not re.fullmatch(r"[0-9a-fA-F]+", payload)
            or len(payload) % 2
        ):
            fail(f"{label}.payload is invalid")
    elif kind == "refresh":
        args = require_exact_keys(args, {"asset"}, label)
        validate_asset(args["asset"], f"{label}.asset")
    elif kind in ("batch_refresh", "ttl_assets"):
        args = require_exact_keys(args, {"oracle", "assets"}, label)
        validate_contract(args["oracle"], f"{label}.oracle")
        assets = args["assets"]
        if not isinstance(assets, list):
            fail(f"{label}.assets must be an array")
        for index, asset in enumerate(assets):
            validate_asset(asset, f"{label}.assets[{index}]")
    elif kind == "ttl_contracts":
        args = require_exact_keys(args, {"contracts"}, label)
        contracts = args["contracts"]
        if not isinstance(contracts, list):
            fail(f"{label}.contracts must be an array")
        for index, contract_id in enumerate(contracts):
            validate_contract(contract_id, f"{label}.contracts[{index}]")
    else:
        fail(f"{label} has an unsupported operation kind")
    return args


def validate_operation(value: object, index: int) -> dict[str, object]:
    label = f"operation[{index}]"
    operation = require_exact_keys(value, OPERATION_KEYS, label)
    number = require_u64(operation["number"], f"{label}.number")
    if number != index + 1:
        fail("operation numbers must be contiguous and one-based")
    phase = operation["phase"]
    if phase not in PHASES:
        fail(f"{label} has an invalid phase")
    kind = operation["kind"]
    if kind not in OPERATION_KINDS:
        fail(f"{label} has an invalid kind")
    allowed_phases = OPERATION_PHASES[kind]
    if phase not in (
        allowed_phases
        if isinstance(allowed_phases, tuple)
        else (allowed_phases,)
    ):
        fail(f"{label} kind is invalid for its phase")
    target = require_string(operation["target"], f"{label}.target")
    if kind == "upload":
        if not SHA256.fullmatch(target):
            fail(f"{label}.target must be a Wasm hash")
    else:
        validate_contract(target, f"{label}.target")
    validate_operation_args(kind, operation["args"], f"{label}.args")
    status = operation["status"]
    if status not in OPERATION_STATUSES:
        fail(f"{label} has an invalid status")
    if not isinstance(operation["tx_hash"], str) or not SHA256.fullmatch(
        operation["tx_hash"]
    ):
        fail(f"{label} lacks a valid prepared transaction hash")
    if operation["envelope_path"] != operation_envelope_path(number).as_posix():
        fail(f"{label} has a noncanonical envelope path")
    if status in TERMINAL_STATUSES:
        if operation["result_path"] != operation_result_path(number).as_posix():
            fail(f"{label} has a noncanonical result path")
    elif operation["result_path"] is not None:
        fail(f"{label} has a result before terminal status")
    return operation


def validate_checkpoint(value: object) -> dict[str, object]:
    checkpoint = require_exact_keys(value, CHECKPOINT_KEYS, "checkpoint")
    if checkpoint["schema_version"] != SCHEMA_VERSION:
        fail("unsupported checkpoint schema version")
    require_u64(checkpoint["revision"], "checkpoint revision")

    context = require_exact_keys(checkpoint["context"], CONTEXT_KEYS, "context")
    network = require_exact_keys(
        context["network"], NETWORK_KEYS, "context.network"
    )
    if network["name"] != NETWORK or network["passphrase"] != PASSPHRASE:
        fail("checkpoint is not bound to Stellar testnet")
    require_string(network["rpc_url"], "context.network.rpc_url")
    validate_account(context["administrator"], "context.administrator")
    if not isinstance(context["git_commit"], str) or not re.fullmatch(
        r"[0-9a-f]{40}", context["git_commit"]
    ):
        fail("context.git_commit is invalid")
    if not isinstance(context["stellar_cli_version"], str) or not re.fullmatch(
        r"[0-9]+\.[0-9]+\.[0-9]+", context["stellar_cli_version"]
    ):
        fail("context.stellar_cli_version is invalid")
    for key in ("manifest_sha256", "deployment_nonce"):
        if not isinstance(context[key], str) or not SHA256.fullmatch(context[key]):
            fail(f"context.{key} is invalid")

    tool_hashes = context["tool_hashes"]
    if not isinstance(tool_hashes, dict) or set(tool_hashes) != TOOL_HASH_KEYS:
        fail("context.tool_hashes has an invalid shape")
    if not all(
        isinstance(digest, str) and SHA256.fullmatch(digest)
        for digest in tool_hashes.values()
    ):
        fail("context.tool_hashes contains an invalid digest")
    artifact_hashes = context["artifact_hashes"]
    if not isinstance(artifact_hashes, dict) or set(artifact_hashes) != set(
        SLUGS
    ):
        fail("context.artifact_hashes has an invalid shape")
    if not all(
        isinstance(digest, str) and SHA256.fullmatch(digest)
        for digest in artifact_hashes.values()
    ):
        fail("context.artifact_hashes contains an invalid digest")

    profile = require_exact_keys(
        context["profile"], PROFILE_KEYS, "context.profile"
    )
    if profile["name"] not in ("xlm", "custom"):
        fail("context.profile.name is invalid")
    for key in ("asset", "base", "reflector_asset", "redstone_asset"):
        validate_asset(profile[key], f"context.profile.{key}")
    require_u32(
        profile["lazer_feed_id"],
        "context.profile.lazer_feed_id",
    )

    freshness = require_exact_keys(
        context["freshness"], FRESHNESS_KEYS, "context.freshness"
    )
    for name in ("reflector", "redstone", "lazer_runtime", "lazer_ingest"):
        window = require_exact_keys(
            freshness[name],
            FRESHNESS_WINDOW_KEYS,
            f"context.freshness.{name}",
        )
        for key, setting in window.items():
            require_u32(
                setting,
                f"context.freshness.{name}.{key}",
                positive=True,
            )
    require_u32(
        freshness["max_cache_age_secs"],
        "context.freshness.max_cache_age_secs",
        positive=True,
    )

    providers = context["providers"]
    expected_providers = {"pyth_verifier", "reflector", "redstone"}
    if not isinstance(providers, dict) or set(providers) != expected_providers:
        fail("context.providers has an invalid shape")
    for name, raw in providers.items():
        provider = require_exact_keys(
            raw, PROVIDER_KEYS, f"context.providers.{name}"
        )
        validate_contract(
            provider["contract_id"], f"context.providers.{name}.contract_id"
        )
        if not isinstance(
            provider["initial_code_hash"], str
        ) or not SHA256.fullmatch(provider["initial_code_hash"]):
            fail(f"context.providers.{name}.initial_code_hash is invalid")

    deployments = checkpoint["deployments"]
    if not isinstance(deployments, dict) or set(deployments) != set(SLUGS):
        fail("deployments do not match the release catalog")
    for slug, raw in deployments.items():
        deployment = require_exact_keys(
            raw, DEPLOYMENT_KEYS, f"deployments.{slug}"
        )
        for key in ("wasm_hash", "salt"):
            if not isinstance(deployment[key], str) or not SHA256.fullmatch(
                deployment[key]
            ):
                fail(f"deployments.{slug}.{key} is invalid")
        validate_contract(
            deployment["contract_id"], f"deployments.{slug}.contract_id"
        )
        if not isinstance(deployment["constructor_args"], dict):
            fail(f"deployments.{slug}.constructor_args must be an object")
        if type(deployment["verified"]) is not bool:
            fail(f"deployments.{slug}.verified must be boolean")

    operations = checkpoint["operations"]
    if not isinstance(operations, list):
        fail("operations must be an array")
    validated_operations = [
        validate_operation(operation, index)
        for index, operation in enumerate(operations)
    ]
    if (
        sum(
            operation["status"] in ("prepared", "submitted")
            for operation in validated_operations
        )
        > 1
    ):
        fail("checkpoint contains more than one unresolved operation")

    phase_results = checkpoint["phase_results"]
    if not isinstance(phase_results, dict) or set(phase_results) != set(PHASES):
        fail("phase_results do not exactly match the rehearsal")
    seen_pending = False
    for phase in PHASES:
        raw = phase_results[phase]
        if raw is None:
            seen_pending = True
            continue
        if seen_pending:
            fail("phase_results must be recorded in order")
        result = require_exact_keys(
            raw, PHASE_RESULT_KEYS, f"phase_results.{phase}"
        )
        numbers = result["operation_numbers"]
        if not isinstance(numbers, list) or len(numbers) != len(
            set(numbers)
        ):
            fail(f"phase_results.{phase}.operation_numbers is invalid")
        for number in numbers:
            require_u64(
                number, f"phase_results.{phase}.operation_numbers"
            )
        for number in numbers:
            if not 1 <= number <= len(validated_operations):
                fail(f"phase_results.{phase} references an unknown operation")
            operation = validated_operations[number - 1]
            if (
                operation["phase"] != phase
                or operation["status"] != "succeeded"
            ):
                fail(f"phase_results.{phase} references an invalid operation")
        if result["evidence_path"] != phase_evidence_path(phase).as_posix():
            fail(f"phase_results.{phase}.evidence_path is noncanonical")
    return checkpoint




@dataclass(frozen=True)
class Settings:
    output: Path
    snapshot: Path
    state_path: Path
    rpc_url: str
    horizon_url: str
    source_identity: str
    administrator: str
    asset_profile: str
    asset: dict[str, str]
    source_assets: dict[str, dict[str, str]]
    feed_id: int
    runtime_policy: dict[str, int]
    ingest_policy: dict[str, int]
    lazer_rest: str

    @property
    def network_args(self) -> list[str]:
        return ["--rpc-url", self.rpc_url, "--network-passphrase", PASSPHRASE]


def env_u32(
    name: str,
    default: int,
    *,
    positive: bool = True,
    maximum: int = UINT32_MAX,
) -> int:
    raw = os.environ.get(name, str(default))
    if not re.fullmatch(r"0|[1-9]\d*", raw):
        fail(f"{name} must be a canonical unsigned decimal integer")
    value = require_u32(int(raw), name, positive=positive)
    if value > maximum:
        fail(f"{name} must not exceed {maximum}")
    return value


def provider_freshness(
    runtime_policy: dict[str, int], provider: str
) -> tuple[int, int]:
    descriptor = PROVIDER_FRESHNESS[provider]
    return (
        runtime_policy[descriptor.age_key],
        runtime_policy[descriptor.drift_key],
    )


def run_simple(
    args: Sequence[str], *, input_text: str | None = None
) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment.pop("PYTH_LAZER_API_KEY", None)
    environment["RUST_LOG"] = "off"
    result = subprocess.run(
        list(args),
        cwd=ROOT,
        input=input_text,
        capture_output=True,
        text=True,
        check=False,
        env=environment,
    )
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        fail(f"{' '.join(args)} failed: {detail}")
    return result


def resolve_administrator(source_identity: str) -> str:
    result = run_simple(["stellar", "keys", "address", source_identity])
    return validate_account(result.stdout.strip(), "source identity address")


def load_settings() -> Settings:
    if os.environ.get("NET", NETWORK) != NETWORK:
        fail("NET must be testnet; production rollout is separate")
    provider_overrides = [
        name for name in PROVIDER_ID_OVERRIDE_KEYS if name in os.environ
    ]
    if provider_overrides:
        fail(
            "testnet provider contract IDs are fixed; remove: "
            + ", ".join(provider_overrides)
        )
    if "LAZER_REST" in os.environ:
        fail("LAZER_REST is fixed for the testnet rehearsal")
    source_identity = os.environ.get("SRC", "")
    if (
        not IDENTITY_RE.fullmatch(source_identity)
        or re.fullmatch(r"S[A-Z2-7]{55}", source_identity)
    ):
        fail("SRC must be a non-secret Stellar CLI identity name")
    administrator = resolve_administrator(source_identity)
    profile = os.environ.get("ASSET_PROFILE", "xlm")
    if profile == "xlm":
        forbidden = ("ASSET_SYMBOL", "REFLECTOR_ASSET_JSON", "REDSTONE_ASSET_JSON", "LAZER_FEED_ID")
        overridden = [name for name in forbidden if name in os.environ]
        if overridden:
            fail(f"xlm profile forbids per-field overrides: {', '.join(overridden)}")
        feed_id = 23
        asset = {"Other": "XLM"}
        source_assets = {
            "reflector": asset,
            "redstone": {"Stellar": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"},
            "lazer": {"Other": str(feed_id)},
        }
    elif profile == "custom":
        required = (
            "ASSET_SYMBOL",
            "REFLECTOR_ASSET_JSON",
            "REDSTONE_ASSET_JSON",
            "LAZER_FEED_ID",
        )
        missing = [name for name in required if name not in os.environ]
        if missing:
            fail(f"custom profile requires: {', '.join(missing)}")
        symbol = os.environ["ASSET_SYMBOL"]
        reflector_asset = strict_json_bytes(
            os.environ["REFLECTOR_ASSET_JSON"].encode(), "REFLECTOR_ASSET_JSON"
        )
        redstone_asset = strict_json_bytes(
            os.environ["REDSTONE_ASSET_JSON"].encode(), "REDSTONE_ASSET_JSON"
        )
        feed_id = env_u32("LAZER_FEED_ID", 0, positive=False)
        asset = validate_asset({"Other": symbol}, "ASSET_SYMBOL")
        source_assets = {
            "reflector": validate_asset(reflector_asset, "REFLECTOR_ASSET_JSON"),
            "redstone": validate_asset(redstone_asset, "REDSTONE_ASSET_JSON"),
            "lazer": validate_asset({"Other": str(feed_id)}, "LAZER_FEED_ID asset"),
        }
    else:
        fail("ASSET_PROFILE must be xlm or custom")
    output = Path(os.environ.get("OUT", ROOT / "target/proxy-oracle-soroban/e2e/testnet"))
    if not output.is_absolute():
        output = ROOT / output
    runtime_policy: dict[str, int] = {}
    for descriptor in PROVIDER_FRESHNESS.values():
        runtime_policy[descriptor.age_key] = env_u32(
            descriptor.age_environment,
            descriptor.default_age,
            maximum=MAX_FRESHNESS_AGE_SECS,
        )
        runtime_policy[descriptor.drift_key] = env_u32(
            descriptor.drift_environment,
            descriptor.default_drift,
            maximum=MAX_FRESHNESS_DRIFT_SECS,
        )
    runtime_policy["max_cache_age_secs"] = env_u32(
        "MAX_CACHE_AGE_SECS",
        600,
        maximum=MAX_FRESHNESS_AGE_SECS,
    )
    ingest_policy = {
        "max_age_secs": env_u32(
            "LAZER_INGEST_MAX_AGE_SECS",
            300,
            maximum=MAX_FRESHNESS_AGE_SECS,
        ),
        "max_clock_drift_secs": env_u32(
            "LAZER_INGEST_MAX_CLOCK_DRIFT_SECS",
            5,
            maximum=MAX_FRESHNESS_DRIFT_SECS,
        ),
    }
    return Settings(
        output=output,
        snapshot=output / "snapshot",
        state_path=output / "state.json",
        rpc_url=os.environ.get("RPC_URL", DEFAULT_RPC_URL),
        horizon_url=os.environ.get("HORIZON_URL", DEFAULT_HORIZON_URL),
        source_identity=source_identity,
        administrator=administrator,
        asset_profile=profile,
        asset=asset,
        source_assets=source_assets,
        feed_id=feed_id,
        runtime_policy=runtime_policy,
        ingest_policy=ingest_policy,
        lazer_rest=LAZER_REST,
    )


def ensure_output_marker(output: Path) -> None:
    marker = output / OUTPUT_MARKER
    if marker.exists() or marker.is_symlink():
        if secure_read(marker, "rehearsal output marker") != OUTPUT_MARKER_CONTENT:
            fail("rehearsal output marker is invalid")
        return
    unexpected = sorted(
        child.name for child in output.iterdir() if child.name != ".lock"
    )
    if unexpected:
        fail("refusing to use a non-empty unmarked rehearsal output directory")
    atomic_write(marker, OUTPUT_MARKER_CONTENT)


@contextlib.contextmanager
def output_lock(output: Path) -> Iterator[None]:
    if output.is_symlink():
        fail("rehearsal output directory must not be a symlink")
    output.mkdir(parents=True, exist_ok=True, mode=0o700)
    metadata = output.stat()
    if not stat.S_ISDIR(metadata.st_mode):
        fail("rehearsal output path is not a directory")
    output.chmod(0o700)
    lock_path = output / ".lock"
    descriptor = os.open(
        lock_path,
        os.O_RDWR
        | os.O_CREAT
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_CLOEXEC", 0),
        0o600,
    )
    try:
        lock_metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(lock_metadata.st_mode)
            or lock_metadata.st_nlink != 1
        ):
            fail("rehearsal lock must be a single-link regular file")
        os.fchmod(descriptor, 0o600)
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        ensure_output_marker(output)
        try:
            yield
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)


def clear_output(output: Path) -> None:
    for child in output.iterdir():
        if child.name in {".lock", OUTPUT_MARKER}:
            continue
        if child.is_dir() and not child.is_symlink():
            shutil.rmtree(child)
        else:
            child.unlink()


def tool_hashes() -> dict[str, str]:
    return {
        relative: sha256_file(ROOT / relative)
        for relative in sorted(TOOL_HASH_KEYS)
    }


def rpc_call(rpc_url: str, method: str, params: object | None = None) -> object:
    request_body: dict[str, object] = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params is not None:
        request_body["params"] = params
    request = urllib.request.Request(
        rpc_url,
        data=canonical_json(request_body),
        headers={
            "Content-Type": "application/json",
            "User-Agent": HTTP_USER_AGENT,
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = response.read()
    except (OSError, urllib.error.URLError) as error:
        fail(f"RPC {method} failed: {error}")
    decoded = strict_json_bytes(payload, f"RPC {method} response")
    if not isinstance(decoded, dict) or decoded.get("jsonrpc") != "2.0" or decoded.get("id") != 1:
        fail(f"RPC {method} returned an invalid envelope")
    if "error" in decoded:
        fail(f"RPC {method} returned an error: {canonical_json_text(decoded['error'])}")
    if set(decoded) != {"jsonrpc", "id", "result"}:
        fail(f"RPC {method} returned unexpected fields")
    return decoded["result"]


def check_network(settings: Settings) -> None:
    result = rpc_call(settings.rpc_url, "getNetwork")
    if not isinstance(result, dict) or result.get("passphrase") != PASSPHRASE:
        fail("RPC endpoint is not Stellar testnet")


def check_funded(settings: Settings) -> None:
    url = f"{settings.horizon_url.rstrip('/')}/accounts/{settings.administrator}"
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            payload = response.read()
    except urllib.error.HTTPError as error:
        if error.code == 404:
            fail("administrator account is not funded on testnet")
        fail(f"Horizon account lookup failed: HTTP {error.code}")
    except OSError as error:
        fail(f"Horizon account lookup failed: {error}")
    account = strict_json_bytes(payload, "Horizon account response")
    if not isinstance(account, dict) or account.get("account_id") != settings.administrator:
        fail("Horizon returned the wrong administrator account")


def fetch_contract_hash(settings: Settings, contract_id: str) -> str:
    with tempfile.TemporaryDirectory(prefix="provider-wasm-") as raw:
        destination = Path(raw) / "contract.wasm"
        run_simple(
            [
                "stellar",
                "contract",
                "fetch",
                *settings.network_args,
                "--id",
                contract_id,
                "--out-file",
                str(destination),
            ]
        )
        if destination.is_symlink() or not destination.is_file():
            fail(f"contract fetch did not produce Wasm for {contract_id}")
        return sha256_file(destination)


def provider_fingerprints(settings: Settings) -> dict[str, dict[str, str]]:
    contracts = {
        "pyth_verifier": PYTH_VERIFIER,
        "reflector": REFLECTOR,
        "redstone": REDSTONE,
    }
    return {
        name: {
            "contract_id": contract_id,
            "initial_code_hash": fetch_contract_hash(settings, contract_id),
        }
        for name, contract_id in contracts.items()
    }


def snapshot_release(settings: Settings) -> tuple[dict[str, object], dict[str, dict[str, str]]]:
    with release_lock():
        manifest_bytes = secure_read(MANIFEST_PATH, "release manifest")
        manifest = strict_json_bytes(manifest_bytes, "release manifest")
        checked = validate_manifest(manifest)
        assert isinstance(manifest, dict)
        settings.snapshot.mkdir(parents=True, exist_ok=True, mode=0o700)
        destination_manifest = settings.snapshot / "release-manifest.json"
        expected_artifacts: dict[str, dict[str, str]] = {}
        staged: list[tuple[Path, bytes, int]] = [(destination_manifest, manifest_bytes, 0o444)]
        manifest_artifacts = manifest["artifacts"]
        assert isinstance(manifest_artifacts, dict)
        for artifact in ARTIFACTS:
            entry = manifest_artifacts[artifact.slug]
            assert isinstance(entry, dict)
            source = ROOT / str(entry["path"])
            payload = secure_read(source, f"release artifact {artifact.slug}")
            if sha256_bytes(payload) != checked[artifact.slug]["sha256"]:
                fail(f"{artifact.slug}: bytes changed after release validation")
            destination = settings.snapshot / artifact.optimized_wasm
            staged.append((destination, payload, 0o444))
            expected_artifacts[artifact.slug] = {
                "path": str(destination.relative_to(settings.output)),
                "sha256": str(checked[artifact.slug]["sha256"]),
                "contract_spec_sha256": str(checked[artifact.slug]["contract_spec_sha256"]),
            }
        for destination, payload, mode in staged:
            if destination.exists():
                if secure_read(destination, f"snapshot {destination.name}") != payload:
                    fail(f"existing snapshot differs: {destination}")
            else:
                atomic_write(destination, payload, mode)
            destination.chmod(mode)
        return manifest, expected_artifacts


def deterministic_salt(context: dict[str, object], slug: str) -> str:
    network = context["network"]
    assert isinstance(network, dict)
    return sha256_bytes(
        canonical_json(
            {
                "deployment_nonce": context["deployment_nonce"],
                "slug": slug,
                "manifest_sha256": context["manifest_sha256"],
                "administrator": context["administrator"],
                "passphrase": network["passphrase"],
            }
        )
    )


def deterministic_contract_id(settings: Settings, salt: str) -> str:
    result = run_simple(
        [
            "stellar",
            "contract",
            "id",
            "wasm",
            *settings.network_args,
            "--source",
            settings.administrator,
            "--salt",
            salt,
        ]
    )
    return validate_contract(result.stdout.strip(), "planned contract ID")


def constructor_args(settings: Settings, contract_ids: dict[str, str], slug: str) -> dict[str, object]:
    base = {"Other": "USD"}
    if slug == "runtime":
        return {"governance": settings.administrator, "base": base}
    if slug == "governance":
        return {"admin": settings.administrator, "proxy_oracle": contract_ids["runtime"], "initial_uniform_ttl_ns": 0}
    if slug == "sep40_adapter":
        return {"owner": settings.administrator, "parent_oracle": contract_ids["runtime"], "asset": settings.asset, "decimals": 8, "resolution": 1, "base": base}
    if slug == "lazer_source":
        return {
            "owner": settings.administrator,
            "config": {
                "verifier": PYTH_VERIFIER,
                "base": base,
                "decimals": 8,
                "channel": "FixedRate200ms",
                "freshness": settings.ingest_policy,
            },
            "supported_feed_ids": [settings.feed_id],
        }
    if slug == "batcher":
        return {}
    fail(f"no constructor schema for {slug}")


def build_context(
    settings: Settings,
    manifest: dict[str, object],
    artifacts: dict[str, dict[str, str]],
    providers: dict[str, dict[str, str]],
    deployment_nonce: str,
) -> dict[str, object]:
    manifest_bytes = secure_read(
        settings.snapshot / "release-manifest.json", "snapshot manifest"
    )
    cli_version, _ = stellar_version_metadata()
    manifest_cli = manifest["stellar_cli"]
    if (
        not isinstance(manifest_cli, dict)
        or cli_version != manifest_cli.get("version")
    ):
        fail("active Stellar CLI version does not match the release manifest")
    freshness = {
        name: {
            "max_age_secs": provider_freshness(
                settings.runtime_policy, provider
            )[0],
            "max_clock_drift_secs": provider_freshness(
                settings.runtime_policy, provider
            )[1],
        }
        for name, provider in (
            ("reflector", "reflector"),
            ("redstone", "redstone"),
            ("lazer_runtime", "lazer"),
        )
    }
    freshness["lazer_ingest"] = dict(settings.ingest_policy)
    freshness["max_cache_age_secs"] = settings.runtime_policy[
        "max_cache_age_secs"
    ]
    return {
        "network": {
            "name": NETWORK,
            "rpc_url": settings.rpc_url,
            "passphrase": PASSPHRASE,
        },
        "administrator": settings.administrator,
        "git_commit": manifest["git_commit"],
        "stellar_cli_version": cli_version,
        "tool_hashes": tool_hashes(),
        "manifest_sha256": sha256_bytes(manifest_bytes),
        "artifact_hashes": {
            slug: entry["sha256"] for slug, entry in artifacts.items()
        },
        "profile": {
            "name": settings.asset_profile,
            "asset": settings.asset,
            "base": {"Other": "USD"},
            "reflector_asset": settings.source_assets["reflector"],
            "redstone_asset": settings.source_assets["redstone"],
            "lazer_feed_id": settings.feed_id,
        },
        "freshness": freshness,
        "providers": providers,
        "deployment_nonce": deployment_nonce,
    }


def deployment_plan(
    settings: Settings, context: dict[str, object]
) -> dict[str, dict[str, object]]:
    salts = {
        slug: deterministic_salt(context, slug)
        for slug in SLUGS
    }
    contract_ids = {
        slug: deterministic_contract_id(settings, salt)
        for slug, salt in salts.items()
    }
    artifact_hashes = context["artifact_hashes"]
    assert isinstance(artifact_hashes, dict)
    plan: dict[str, dict[str, object]] = {}
    for slug in SLUGS:
        plan[slug] = {
            "wasm_hash": artifact_hashes[slug],
            "salt": salts[slug],
            "contract_id": contract_ids[slug],
            "constructor_args": constructor_args(settings, contract_ids, slug),
        }
    return plan


def validate_deployment_plan(
    settings: Settings,
    context: dict[str, object],
    deployments: object,
) -> None:
    if not isinstance(deployments, dict):
        fail("deployments must be an object")
    expected = deployment_plan(settings, context)
    for slug in SLUGS:
        actual = deployments[slug]
        assert isinstance(actual, dict)
        immutable = {key: actual[key] for key in DEPLOYMENT_PLAN_KEYS}
        if immutable != expected[slug]:
            fail(f"deployments.{slug} drifted from its deterministic plan")


def initialize_checkpoint(
    settings: Settings, context: dict[str, object]
) -> dict[str, object]:
    deployments = {
        slug: {**entry, "verified": False}
        for slug, entry in deployment_plan(settings, context).items()
    }
    checkpoint: dict[str, object] = {
        "schema_version": SCHEMA_VERSION,
        "revision": 0,
        "context": context,
        "deployments": deployments,
        "operations": [],
        "phase_results": {phase: None for phase in PHASES},
    }
    return validate_checkpoint(checkpoint)


class CheckpointStore:
    def __init__(self, path: Path, value: dict[str, object]):
        self.path = path
        self.value = validate_checkpoint(value)

    @classmethod
    def load(cls, path: Path) -> "CheckpointStore":
        return cls(path, validate_checkpoint(strict_json_bytes(secure_read(path, "checkpoint"), "checkpoint")))

    def save(self, expected_revision: int) -> None:
        if self.value["revision"] != expected_revision:
            fail("checkpoint revision changed unexpectedly in memory")
        if self.path.exists():
            on_disk = validate_checkpoint(strict_json_bytes(secure_read(self.path, "checkpoint"), "checkpoint"))
            if on_disk["revision"] != expected_revision:
                fail("checkpoint revision conflict")
        elif expected_revision != 0:
            fail("checkpoint disappeared during update")
        self.value["revision"] = expected_revision + 1
        validate_checkpoint(self.value)
        atomic_write(self.path, json.dumps(self.value, indent=2, sort_keys=False).encode() + b"\n")

    def first_save(self) -> None:
        if self.path.exists():
            fail("checkpoint already exists")
        validate_checkpoint(self.value)
        atomic_write(self.path, json.dumps(self.value, indent=2, sort_keys=False).encode() + b"\n")

    def mutate(self, change: Callable[[dict[str, object]], None]) -> None:
        revision = int(self.value["revision"])
        change(self.value)
        self.save(revision)

    def operations(self) -> list[dict[str, object]]:
        operations = self.value["operations"]
        assert isinstance(operations, list)
        return operations

    def failed_operation(
        self,
        phase: str,
        kind: str,
        target: str,
        args: dict[str, object],
    ) -> dict[str, object] | None:
        return next(
            (
                operation
                for operation in self.operations()
                if operation["phase"] == phase
                and operation["kind"] == kind
                and operation["target"] == target
                and operation["args"] == args
                and operation["status"] == "failed"
            ),
            None,
        )

    def unresolved(self) -> dict[str, object] | None:
        found = None
        for operation in self.operations():
            if operation["status"] not in ("prepared", "submitted"):
                continue
            if found is not None:
                fail("more than one operation is unresolved")
            found = operation
        return found

    def successful_operation(
        self,
        phase: str,
        kind: str,
        target: str,
        args: dict[str, object],
    ) -> dict[str, object] | None:
        return next(
            (
                operation
                for operation in self.operations()
                if operation["phase"] == phase
                and operation["kind"] == kind
                and operation["target"] == target
                and operation["args"] == args
                and operation["status"] == "succeeded"
            ),
            None,
        )

    def record_prepared(
        self,
        phase: str,
        kind: str,
        target: str,
        args: dict[str, object],
        tx_hash: str,
        envelope_path: Path,
    ) -> dict[str, object]:
        if self.unresolved() is not None:
            fail("cannot prepare a second unresolved operation")
        operation: dict[str, object] = {
            "number": len(self.operations()) + 1,
            "phase": phase,
            "kind": kind,
            "target": target,
            "args": args,
            "status": "prepared",
            "tx_hash": tx_hash,
            "envelope_path": str(envelope_path.relative_to(self.path.parent)),
            "result_path": None,
        }
        self.mutate(
            lambda value: value["operations"].append(operation)  # type: ignore[union-attr]
        )
        return operation

    def mark_submitted(self, operation: dict[str, object]) -> None:
        if operation["status"] != "prepared":
            fail(
                "invalid operation transition "
                f"{operation['status']} -> submitted"
            )
        self.mutate(
            lambda _: operation.__setitem__("status", "submitted")
        )

    def terminal_result_path(
        self, operation: dict[str, object], result_path: Path
    ) -> str:
        if operation["status"] != "submitted":
            fail(
                "cannot complete operation from "
                f"{operation['status']} status"
            )
        try:
            relative = result_path.relative_to(self.path.parent)
        except ValueError:
            fail("operation result path escapes the rehearsal directory")
        expected = operation_result_path(int(operation["number"]))
        if relative != expected:
            fail("operation result path is noncanonical")
        return relative.as_posix()

    def mark_succeeded(
        self, operation: dict[str, object], result_path: Path
    ) -> None:
        relative = self.terminal_result_path(operation, result_path)

        def change(_: dict[str, object]) -> None:
            operation["status"] = "succeeded"
            operation["result_path"] = relative

        self.mutate(change)

    def mark_failed(
        self,
        operation: dict[str, object],
        result_path: Path,
        error: str,
    ) -> None:
        relative = self.terminal_result_path(operation, result_path)
        if not error:
            fail("failed operation requires an error")

        def change(_: dict[str, object]) -> None:
            operation["status"] = "failed"
            operation["result_path"] = relative

        self.mutate(change)

    def deployment(self, slug: str) -> dict[str, object]:
        deployments = self.value["deployments"]
        assert isinstance(deployments, dict)
        deployment = deployments[slug]
        assert isinstance(deployment, dict)
        return deployment

    def set_deployment_verified(self, slug: str) -> None:
        deployment = self.deployment(slug)
        if deployment["verified"] is True:
            return
        self.mutate(lambda _: deployment.__setitem__("verified", True))

    def pass_phase(self, phase: str, evidence: list[object]) -> None:
        phase_results = self.value["phase_results"]
        assert isinstance(phase_results, dict)
        if phase_results[phase] is not None:
            return
        phase_index = PHASES.index(phase)
        if any(
            phase_results[prior] is None for prior in PHASES[:phase_index]
        ):
            fail(f"cannot pass {phase} before earlier phases")
        relative = phase_evidence_path(phase)
        evidence_path = self.path.parent / relative
        atomic_write(
            evidence_path,
            json.dumps(evidence, indent=2, sort_keys=True).encode() + b"\n",
        )
        operation_numbers = [
            int(operation["number"])
            for operation in self.operations()
            if operation["phase"] == phase
            and operation["status"] == "succeeded"
        ]
        result = {
            "operation_numbers": operation_numbers,
            "evidence_path": relative.as_posix(),
        }
        self.mutate(
            lambda _: phase_results.__setitem__(phase, result)
        )


class TransactionExecutor:
    def __init__(
        self,
        settings: Settings,
        store: CheckpointStore,
        *,
        runner: Callable[..., subprocess.CompletedProcess[str]] = run_simple,
        rpc: Callable[[str, str, object | None], object] = rpc_call,
        sleep: Callable[[float], None] = time.sleep,
    ):
        self.settings = settings
        self.store = store
        self.runner = runner
        self.rpc = rpc
        self.sleep = sleep

    def write_transcript(self, operation_number: int, step: str, result: subprocess.CompletedProcess[str]) -> None:
        directory = self.settings.output / operation_directory(operation_number)
        atomic_write(directory / f"{step}.stdout", result.stdout.encode())
        atomic_write(directory / f"{step}.stderr", result.stderr.encode())

    def command(self, args: list[str], *, input_text: str | None, operation_number: int, step: str) -> str:
        result = self.runner(args, input_text=input_text)
        self.write_transcript(operation_number, step, result)
        output = result.stdout.strip()
        if not output:
            fail(f"{step} returned empty stdout")
        return output

    def decode_envelope(
        self,
        envelope: str,
        operation_number: int,
        step: str,
    ) -> dict[str, object]:
        output = self.command(
            ["stellar", "tx", "decode", "--output", "json-formatted"],
            input_text=envelope,
            operation_number=operation_number,
            step=step,
        )
        decoded = strict_json_bytes(output.encode(), step)
        if not isinstance(decoded, dict):
            fail(f"{step} must decode to an object")
        return decoded

    def decode_transaction(
        self,
        envelope: str,
        operation_number: int,
        step: str,
    ) -> tuple[dict[str, object], dict[str, object]]:
        outer = require_exact_keys(
            self.decode_envelope(envelope, operation_number, step),
            {"tx"},
            step,
        )
        inner = require_exact_keys(
            outer["tx"],
            {"tx", "signatures"},
            f"{step}.tx",
        )
        transaction = inner["tx"]
        if not isinstance(transaction, dict):
            fail(f"{step}.tx.tx must be an object")
        return outer, transaction

    def transaction_hash(
        self,
        envelope: str,
        operation_number: int,
        step: str,
    ) -> str:
        decoded = self.decode_envelope(
            envelope,
            operation_number,
            f"{step}-decode",
        )
        if set(decoded) == {"tx"}:
            transaction_envelope = require_exact_keys(
                decoded["tx"],
                {"tx", "signatures"},
                f"{step}.tx",
            )
            signatures = transaction_envelope["signatures"]
            if not isinstance(signatures, list) or not signatures:
                fail(f"{step} transaction must be signed")
            return self.command(
                ["stellar", "tx", "hash", *self.settings.network_args],
                input_text=envelope,
                operation_number=operation_number,
                step=step,
            )

        outer = require_exact_keys(decoded, {"tx_fee_bump"}, step)
        fee_bump_envelope = require_exact_keys(
            outer["tx_fee_bump"],
            {"tx", "signatures"},
            f"{step}.tx_fee_bump",
        )
        signatures = fee_bump_envelope["signatures"]
        if not isinstance(signatures, list) or not signatures:
            fail(f"{step} fee-bump transaction must be signed")
        fee_bump_transaction = fee_bump_envelope["tx"]
        if not isinstance(fee_bump_transaction, dict):
            fail(f"{step} fee-bump transaction must be an object")
        signature_payload = {
            "network_id": sha256_bytes(PASSPHRASE.encode()),
            "tagged_transaction": {
                "tx_fee_bump": fee_bump_transaction,
            },
        }
        encoded_payload = self.command(
            [
                "stellar",
                "xdr",
                "encode",
                "--type",
                "TransactionSignaturePayload",
                "--output",
                "single-base64",
            ],
            input_text=canonical_json_text(signature_payload),
            operation_number=operation_number,
            step=f"{step}-payload",
        )
        try:
            payload = base64.b64decode(encoded_payload, validate=True)
        except (binascii.Error, ValueError):
            fail(f"{step} signature payload is not canonical base64")
        return sha256_bytes(payload)

    def wrap_fee_bump_if_required(
        self,
        unsigned: str,
        simulated: str,
        signed_inner: str,
        operation_number: int,
    ) -> str:
        _, simulated_transaction = self.decode_transaction(
            simulated,
            operation_number,
            "fee-check",
        )
        simulated_fee = require_u32(
            simulated_transaction.get("fee"),
            "simulated transaction fee",
        )
        if simulated_fee != 0:
            return signed_inner

        extension = require_exact_keys(
            simulated_transaction.get("ext"),
            {"v1"},
            "simulated transaction extension",
        )
        version_one = extension["v1"]
        if not isinstance(version_one, dict):
            fail("simulated transaction v1 extension must be an object")
        resource_fee = require_integer(
            version_one.get("resource_fee"),
            "simulated transaction resource fee",
        )
        if resource_fee <= 0:
            fail("simulated transaction resource fee must be positive")

        _, unsigned_transaction = self.decode_transaction(
            unsigned,
            operation_number,
            "fee-bump-decode-unsigned",
        )
        inclusion_fee = require_u32(
            unsigned_transaction.get("fee"),
            "unsigned transaction inclusion fee",
            positive=True,
        )
        fee_bump_fee = resource_fee + 2 * inclusion_fee
        if fee_bump_fee <= UINT32_MAX or fee_bump_fee > INT64_MAX:
            fail("simulated zero-fee transaction has an invalid fee-bump amount")

        signed_envelope, signed_transaction = self.decode_transaction(
            signed_inner,
            operation_number,
            "fee-bump-decode-inner",
        )
        if (
            signed_transaction.get("fee") != 0
            or signed_transaction.get("source_account")
            != self.settings.administrator
        ):
            fail("signed fee-bump inner transaction differs from its simulation")
        signed_transaction_envelope = signed_envelope["tx"]
        assert isinstance(signed_transaction_envelope, dict)
        signatures = signed_transaction_envelope["signatures"]
        if not isinstance(signatures, list) or not signatures:
            fail("fee-bump inner transaction must be signed")

        fee_bump = {
            "tx_fee_bump": {
                "tx": {
                    "fee_source": self.settings.administrator,
                    "fee": str(fee_bump_fee),
                    "inner_tx": signed_envelope,
                    "ext": "v0",
                },
                "signatures": [],
            }
        }
        encoded = self.command(
            ["stellar", "tx", "encode"],
            input_text=canonical_json_text(fee_bump),
            operation_number=operation_number,
            step="fee-bump-encode",
        )
        return self.command(
            [
                "stellar",
                "tx",
                "sign",
                *self.settings.network_args,
                "--sign-with-key",
                self.settings.source_identity,
            ],
            input_text=encoded,
            operation_number=operation_number,
            step="fee-bump-sign",
        )

    def poll(self, operation: dict[str, object], attempts: int = 60) -> bool:
        tx_hash = str(operation["tx_hash"])
        result_path = self.settings.output / operation_result_path(
            int(operation["number"])
        )
        for attempt in range(attempts):
            result = self.rpc(self.settings.rpc_url, "getTransaction", {"hash": tx_hash})
            if not isinstance(result, dict) or not isinstance(result.get("status"), str):
                fail("getTransaction returned an invalid result")
            status = result["status"]
            if status in ("SUCCESS", "FAILED"):
                atomic_write(result_path, json.dumps(result, indent=2, sort_keys=True).encode() + b"\n")
                if status == "SUCCESS":
                    self.store.mark_succeeded(operation, result_path)
                    return True
                diagnostic = canonical_json_text(result)
                self.store.mark_failed(operation, result_path, diagnostic)
                fail(f"transaction {tx_hash} failed: {diagnostic}")
            if status not in ("NOT_FOUND", "PENDING"):
                fail(f"getTransaction returned unknown status {status!r}")
            if attempt + 1 < attempts:
                self.sleep(2)
        return False

    def operation_path(self, operation: dict[str, object], key: str) -> Path:
        raw = require_string(operation[key], f"operation.{key}")
        relative = Path(raw)
        if relative.is_absolute() or ".." in relative.parts:
            fail(f"operation.{key} escapes the rehearsal directory")
        path = self.settings.output / relative
        output = self.settings.output.resolve()
        parent = path.parent.resolve()
        if parent != output and output not in parent.parents:
            fail(f"operation.{key} escapes the rehearsal directory")
        return path
    def verify_envelope_hash(
        self,
        operation: dict[str, object],
        envelope: str,
    ) -> None:
        operation_number = int(operation["number"])
        actual = self.transaction_hash(
            envelope,
            operation_number,
            "verify-hash",
        )
        if actual != operation["tx_hash"]:
            fail("signed transaction envelope does not match its checkpoint hash")


    def submit(self, operation: dict[str, object]) -> None:
        if operation["status"] != "prepared":
            fail("only a prepared operation may be submitted")
        envelope = secure_read(
            self.operation_path(operation, "envelope_path"),
            "signed transaction envelope",
        ).decode().strip()
        self.verify_envelope_hash(operation, envelope)
        self.store.mark_submitted(operation)
        operation_number = int(operation["number"])
        operation_dir = self.settings.output / operation_directory(
            operation_number
        )
        try:
            result = self.runner(
                ["stellar", "tx", "send", *self.settings.network_args],
                input_text=envelope,
            )
            self.write_transcript(operation_number, "send", result)
        except RehearsalError as error:
            atomic_write(
                operation_dir / "send.error", (str(error) + "\n").encode()
            )

    def resolve_unfinished(self) -> None:
        operation = self.store.unresolved()
        if operation is None:
            return
        if operation["status"] == "prepared":
            self.submit(operation)
        elif operation["status"] != "submitted":
            fail("unresolved operation has an invalid status")
        if not self.poll(operation):
            fail(
                f"submitted transaction {operation['tx_hash']} "
                "remains unresolved"
            )

    def result(self, operation: dict[str, object]) -> dict[str, object]:
        if operation["status"] != "succeeded":
            fail("transaction result requested before success")
        value = strict_json_bytes(
            secure_read(
                self.operation_path(operation, "result_path"),
                "transaction result",
            ),
            "transaction result",
        )
        if not isinstance(value, dict) or value.get("status") != "SUCCESS":
            fail("persisted transaction result is not successful")
        return value

    def return_value(self, operation: dict[str, object]) -> object:
        stored = self.result(operation)
        encoded = stored.get("returnValue")
        if isinstance(encoded, str) and encoded:
            decoded = self.runner(
                [
                    "stellar",
                    "xdr",
                    "decode",
                    "--type",
                    "ScVal",
                    "--input",
                    "single-base64",
                    "--output",
                    "json-formatted",
                ],
                input_text=encoded,
            )
            return strict_json_bytes(
                decoded.stdout.encode(), "transaction returnValue"
            )

        metadata_xdr = stored.get("resultMetaXdr")
        if not isinstance(metadata_xdr, str) or not metadata_xdr:
            fail("successful transaction lacks result metadata")
        decoded = self.runner(
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
            input_text=metadata_xdr,
        )
        metadata = strict_json_bytes(
            decoded.stdout.encode(), "transaction result metadata"
        )
        if not isinstance(metadata, dict) or len(metadata) != 1:
            fail("transaction result metadata has an invalid version")
        version, body = next(iter(metadata.items()))
        if version not in {"v3", "v4"} or not isinstance(body, dict):
            fail("transaction result metadata is not Soroban metadata")
        soroban = body.get("soroban_meta")
        if not isinstance(soroban, dict) or "return_value" not in soroban:
            fail("transaction result metadata lacks a return value")
        return soroban["return_value"]

    def execute(
        self,
        phase: str,
        kind: str,
        target: str,
        args: dict[str, object],
        build_args: list[str],
    ) -> dict[str, object]:
        successful = self.store.successful_operation(
            phase, kind, target, args
        )
        if successful is not None:
            return successful
        failed = self.store.failed_operation(
            phase, kind, target, args
        )
        if failed is not None:
            fail(
                f"operation {failed['number']} previously failed; "
                "automatic retry is forbidden"
            )
        self.resolve_unfinished()
        successful = self.store.successful_operation(
            phase, kind, target, args
        )
        if successful is not None:
            return successful
        failed = self.store.failed_operation(
            phase, kind, target, args
        )
        if failed is not None:
            fail(
                f"operation {failed['number']} failed; "
                "automatic retry is forbidden"
            )
        operation_number = len(self.store.operations()) + 1
        unsigned = self.command(build_args, input_text=None, operation_number=operation_number, step="build")
        simulated = self.command(
            ["stellar", "tx", "simulate", *self.settings.network_args, "--source", self.settings.administrator],
            input_text=unsigned,
            operation_number=operation_number,
            step="simulate",
        )
        signed_inner = self.command(
            ["stellar", "tx", "sign", *self.settings.network_args, "--sign-with-key", self.settings.source_identity],
            input_text=simulated,
            operation_number=operation_number,
            step="sign",
        )
        signed = self.wrap_fee_bump_if_required(
            unsigned,
            simulated,
            signed_inner,
            operation_number,
        )
        tx_hash = self.transaction_hash(
            signed,
            operation_number,
            "hash",
        )
        if not SHA256.fullmatch(tx_hash):
            fail("stellar tx hash did not return a lowercase SHA-256 digest")
        envelope_path = self.settings.output / operation_envelope_path(
            operation_number
        )
        atomic_write(envelope_path, signed.encode() + b"\n")
        operation = self.store.record_prepared(
            phase,
            kind,
            target,
            args,
            tx_hash,
            envelope_path,
        )
        self.submit(operation)
        if not self.poll(operation):
            fail(f"submitted transaction {tx_hash} remains unresolved")
        return operation


class Rehearsal:
    def __init__(self, settings: Settings, store: CheckpointStore):
        self.settings = settings
        self.store = store
        self.transactions = TransactionExecutor(settings, store)

    @property
    def deployments(self) -> dict[str, dict[str, object]]:
        value = self.store.value["deployments"]
        assert isinstance(value, dict)
        return value  # type: ignore[return-value]

    def build_upload(self, slug: str) -> list[str]:
        artifact = catalog_artifact(slug)
        return [
            "stellar",
            "contract",
            "upload",
            *self.settings.network_args,
            "--source",
            self.settings.administrator,
            "--wasm",
            str(self.settings.snapshot / artifact.optimized_wasm),
            "--build-only",
        ]

    def constructor_cli(self, slug: str) -> list[str]:
        arguments = self.deployments[slug]["constructor_args"]
        assert isinstance(arguments, dict)
        output: list[str] = []
        for name, value in arguments.items():
            output.extend((f"--{name}", canonical_json_text(value) if isinstance(value, (dict, list)) else str(value)))
        return output

    def build_deploy(self, slug: str) -> list[str]:
        deployment = self.deployments[slug]
        artifact = catalog_artifact(slug)
        command = [
            "stellar", "contract", "deploy", *self.settings.network_args,
            "--source", self.settings.administrator,
            "--wasm", str(self.settings.snapshot / artifact.optimized_wasm),
            "--salt", str(deployment["salt"]),
            "--build-only",
        ]
        constructor = self.constructor_cli(slug)
        if constructor:
            command.extend(("--", *constructor))
        return command

    def build_invoke(self, contract_id: str, function: str, arguments: dict[str, object]) -> list[str]:
        command = [
            "stellar", "contract", "invoke", *self.settings.network_args,
            "--source", self.settings.administrator,
            "--id", contract_id,
            "--build-only", "--", function,
        ]
        for name, value in arguments.items():
            command.extend((f"--{name}", canonical_json_text(value) if isinstance(value, (dict, list)) else str(value)))
        return command

    def view(
        self,
        contract_id: str,
        function: str,
        arguments: dict[str, object] | None = None,
        *,
        decoder: Callable[[object, str], T],
    ) -> T:
        command = [
            "stellar",
            "contract",
            "invoke",
            *self.settings.network_args,
            "--source",
            self.settings.administrator,
            "--id",
            contract_id,
            "--send",
            "no",
            "--",
            function,
        ]
        for name, value in (arguments or {}).items():
            command.extend(
                (
                    f"--{name}",
                    canonical_json_text(value)
                    if isinstance(value, (dict, list))
                    else str(value),
                )
            )
        result = run_simple(command)
        label = f"{function} view"
        return decoder(strict_json_bytes(result.stdout.strip().encode(), label), label)

    def sep40_price(
        self, contract_id: str, asset: dict[str, str]
    ) -> dict[str, object] | None:
        built = run_simple(
            self.build_invoke(contract_id, "lastprice", {"asset": asset})
        )
        transaction = require_string(
            built.stdout.strip(), "lastprice transaction"
        )
        simulation = rpc_call(
            self.settings.rpc_url,
            "simulateTransaction",
            {"transaction": transaction},
        )
        if not isinstance(simulation, dict):
            fail("lastprice simulation returned an invalid result")
        if "error" in simulation:
            fail(
                "lastprice simulation failed: "
                + canonical_json_text(simulation["error"])
            )
        results = simulation.get("results")
        if (
            not isinstance(results, list)
            or len(results) != 1
            or not isinstance(results[0], dict)
        ):
            fail("lastprice simulation did not return exactly one result")
        encoded = require_string(
            results[0].get("xdr"), "lastprice simulation result XDR"
        )
        decoded = run_simple(
            [
                "stellar",
                "xdr",
                "decode",
                "--type",
                "ScVal",
                "--input",
                "single-base64",
                "--output",
                "json-formatted",
            ],
            input_text=encoded,
        )
        label = "lastprice view"
        return decode_scval_optional_price(
            strict_json_bytes(decoded.stdout.encode(), label), label
        )

    def ownership_handoff_recorded(self) -> bool:
        governance = str(
            self.deployments["governance"]["contract_id"]
        )
        create_ids = {
            require_u64(
                entry["args"]["id"],  # type: ignore[index]
                "ownership proposal ID",
            )
            for entry in self.store.operations()
            if entry["phase"] == "ownership"
            and entry["kind"] == "proposal_create"
            and entry["target"] == governance
            and entry["status"] == "succeeded"
            and isinstance(entry["args"], dict)
            and entry["args"].get("operation") == "AcceptOwnership"
        }
        return any(
            entry["phase"] == "ownership"
            and entry["kind"] == "proposal_execute"
            and entry["target"] == governance
            and entry["status"] == "succeeded"
            and isinstance(entry["args"], dict)
            and entry["args"].get("id") in create_ids
            for entry in self.store.operations()
        )

    def verify_deployment_postconditions(self, slug: str) -> None:
        contract_id = str(self.deployments[slug]["contract_id"])
        administrator = self.settings.administrator
        runtime = str(self.deployments["runtime"]["contract_id"])
        governance = str(
            self.deployments["governance"]["contract_id"]
        )
        if slug == "runtime":
            base = self.view(
                contract_id,
                "source_base",
                decoder=decode_optional_asset,
            )
            expected_base = self.deployments[slug][
                "constructor_args"
            ]["base"]  # type: ignore[index]
            if base != expected_base:
                fail("runtime source base differs from its constructor")
            expected_owner = (
                governance
                if self.ownership_handoff_recorded()
                else administrator
            )
            owner = self.view(
                contract_id, "get_owner", decoder=validate_address
            )
            if owner != expected_owner:
                fail("runtime owner differs from its recorded lifecycle")
            return
        if slug == "governance":
            linked_runtime = self.view(
                contract_id,
                "proxy_oracle",
                decoder=validate_contract,
            )
            if linked_runtime != runtime:
                fail("governance points at the wrong runtime")
            has_admin = self.view(
                contract_id,
                "has_role",
                {"account": administrator, "role": "Admin"},
                decoder=require_boolean,
            )
            if not has_admin:
                fail("governance administrator lacks the Admin role")
            active_ids = self.view(
                contract_id,
                "active_ids",
                decoder=decode_u64_list,
            )
            if active_ids:
                fail("governance has unexpected active proposals")
            for kind in GOVERNANCE_OPERATION_KINDS:
                ttl = self.view(
                    contract_id,
                    "get_operation_ttl",
                    {"kind": kind},
                    decoder=decode_nonnegative_integer,
                )
                if ttl != 0:
                    fail(
                        "governance operation TTL differs from "
                        "the zero-TTL rehearsal plan"
                    )
            return
        if slug == "lazer_source":
            arguments = self.deployments[slug]["constructor_args"]
            assert isinstance(arguments, dict)
            config = self.view(
                contract_id,
                "config",
                decoder=decode_optional_lazer_config,
            )
            if config != arguments["config"]:
                fail("Lazer source config differs from its constructor")
            feed_ids = self.view(
                contract_id,
                "supported_feed_ids",
                decoder=decode_u32_list,
            )
            if feed_ids != [self.settings.feed_id]:
                fail("Lazer source feed registry differs from its constructor")
            epoch = self.view(
                contract_id,
                "verification_epoch",
                decoder=decode_nonnegative_integer,
            )
            decimals = self.view(
                contract_id, "decimals", decoder=require_u32
            )
            resolution = self.view(
                contract_id, "resolution", decoder=require_u32
            )
            owner = self.view(
                contract_id, "get_owner", decoder=validate_address
            )
            if (
                epoch != 0
                or decimals != 8
                or resolution != 1
                or owner != administrator
            ):
                fail("Lazer source constructor postconditions do not hold")
            return
        if slug == "sep40_adapter":
            arguments = self.deployments[slug]["constructor_args"]
            assert isinstance(arguments, dict)
            expected = {
                key: value
                for key, value in arguments.items()
                if key != "owner"
            }
            config = self.view(
                contract_id,
                "config",
                decoder=decode_optional_adapter_config,
            )
            owner = self.view(
                contract_id, "get_owner", decoder=validate_address
            )
            if config != expected or owner != administrator:
                fail("adapter config differs from its constructor")
            return
        if slug != "batcher":
            fail(f"no deployment postconditions for {slug}")

    def verify_deployment(self, slug: str) -> None:
        deployment = self.deployments[slug]
        actual = fetch_contract_hash(
            self.settings, str(deployment["contract_id"])
        )
        if actual != deployment["wasm_hash"]:
            fail(f"{slug} deployed Wasm hash mismatch")
        self.verify_deployment_postconditions(slug)
        self.store.set_deployment_verified(slug)

    def verify_recorded_deployments(self) -> None:
        for slug in DEPLOYMENT_ORDER:
            if self.deployments[slug]["verified"]:
                self.verify_deployment(slug)

    def verify_provider_fingerprints(self) -> None:
        context = self.store.value["context"]
        assert isinstance(context, dict)
        providers = context["providers"]
        assert isinstance(providers, dict)
        for name in ("pyth_verifier", "reflector", "redstone"):
            provider = providers[name]
            assert isinstance(provider, dict)
            contract_id = str(provider["contract_id"])
            actual = fetch_contract_hash(self.settings, contract_id)
            if actual != provider["initial_code_hash"]:
                fail(
                    f"{name} provider code drifted; "
                    "start a fresh rehearsal output"
                )

    def latest_ledger(self) -> int:
        result = rpc_call(self.settings.rpc_url, "getLatestLedger")
        if not isinstance(result, dict):
            fail("getLatestLedger returned an invalid result")
        return require_u32(
            result.get("sequence"),
            "getLatestLedger sequence",
            positive=True,
        )

    def proposal_at(
        self, governance: str, proposal_id: int
    ) -> dict[str, object] | None:
        return self.view(
            governance,
            "get_proposal",
            {"id": proposal_id},
            decoder=decode_optional_proposal,
        )

    def validate_recorded_proposal(
        self,
        proposal: dict[str, object],
        operation: object,
        label: str,
    ) -> None:
        if (
            proposal["operation"] != operation
            or proposal["created_by"] != self.settings.administrator
            or proposal["ttl_ns"] != 0
        ):
            fail(f"{label} proposal conflicts with the recorded action")

    def recorded_proposal_state(
        self, phase: str, label: str, operation: object
    ) -> tuple[list[int], bool, int]:
        governance = str(
            self.deployments["governance"]["contract_id"]
        )
        creates = []
        for entry in self.store.operations():
            if (
                entry["phase"] != phase
                or entry["kind"] != "proposal_create"
                or entry["target"] != governance
            ):
                continue
            arguments = entry["args"]
            assert isinstance(arguments, dict)
            if arguments.get("operation") != operation:
                fail(f"{label} has a conflicting recorded proposal")
            if entry["status"] == "failed":
                fail(
                    f"{label} proposal creation previously failed; "
                    "automatic retry is forbidden"
                )
            if entry["status"] != "succeeded":
                fail(f"{label} proposal creation remains unresolved")
            if (
                arguments.get("caller")
                != self.settings.administrator
                or arguments.get("requested_ttl") != 0
            ):
                fail(f"{label} proposal checkpoint arguments conflict")
            creates.append(
                require_u64(
                    arguments.get("id"), f"{label} proposal ID"
                )
            )
        if len(set(creates)) != len(creates):
            fail(f"{label} proposal ID is recorded more than once")
        active: list[int] = []
        consumed = False
        for proposal_id in creates:
            execute_args = {
                "caller": self.settings.administrator,
                "id": proposal_id,
            }
            executed = self.store.successful_operation(
                phase,
                "proposal_execute",
                governance,
                execute_args,
            )
            if executed is not None:
                fail(
                    f"{label} proposal execution is already recorded "
                    "but its postcondition is absent"
                )
            proposal = self.proposal_at(governance, proposal_id)
            if proposal is None:
                consumed = True
                continue
            self.validate_recorded_proposal(
                proposal, operation, label
            )
            active.append(proposal_id)
        if len(active) > 1:
            fail(f"multiple active proposals recorded for {label}")
        chain_active = self.view(
            governance,
            "active_ids",
            decoder=decode_u64_list,
        )
        if chain_active != sorted(active):
            fail(
                f"{label} has unrecorded or missing active proposals"
            )
        return active, consumed, len(creates)

    def run_governance_operation(
        self,
        phase: str,
        label: str,
        operation: object,
        *,
        allow_recreate_missing: bool = False,
    ) -> None:
        governance = str(
            self.deployments["governance"]["contract_id"]
        )
        active, consumed, create_count = (
            self.recorded_proposal_state(phase, label, operation)
        )
        if active:
            if consumed and not allow_recreate_missing:
                fail(f"{label} has consumed or cancelled proposal history")
            proposal_id = active[0]
            next_id = self.view(
                governance,
                "next_proposal_id",
                decoder=decode_nonnegative_integer,
            )
            if next_id != proposal_id + 1:
                fail(f"{label} proposal ID no longer matches next ID")
        elif create_count:
            if consumed and not allow_recreate_missing:
                fail(f"{label} proposal was consumed or cancelled")
            proposal_id = self.view(
                governance,
                "next_proposal_id",
                decoder=decode_nonnegative_integer,
            )
        else:
            proposal_id = self.view(
                governance,
                "next_proposal_id",
                decoder=decode_nonnegative_integer,
            )
        create_args = {
            "caller": self.settings.administrator,
            "id": proposal_id,
            "operation": operation,
            "requested_ttl": 0,
        }
        if not active:
            cli_args = dict(create_args)
            if isinstance(operation, str):
                cli_args["operation"] = canonical_json_text(operation)
            self.transactions.execute(
                phase,
                "proposal_create",
                governance,
                create_args,
                self.build_invoke(
                    governance,
                    "create_proposal",
                    cli_args,
                ),
            )
            proposal = self.proposal_at(governance, proposal_id)
            if proposal is None:
                fail(f"{label} proposal is absent after creation")
            self.validate_recorded_proposal(
                proposal, operation, label
            )
            next_id = self.view(
                governance,
                "next_proposal_id",
                decoder=decode_nonnegative_integer,
            )
            if next_id != proposal_id + 1:
                fail(
                    f"{label} proposal creation advanced the wrong ID"
                )
            active_ids = self.view(
                governance,
                "active_ids",
                decoder=decode_u64_list,
            )
            if active_ids != [proposal_id]:
                fail(
                    f"{label} proposal is not the sole active proposal"
                )
        execute_args = {
            "caller": self.settings.administrator,
            "id": proposal_id,
        }
        self.transactions.execute(
            phase,
            "proposal_execute",
            governance,
            execute_args,
            self.build_invoke(
                governance,
                "execute_proposal",
                execute_args,
            ),
        )
        if self.proposal_at(governance, proposal_id) is not None:
            fail(f"{label} proposal remains active after execution")
        active_ids = self.view(
            governance,
            "active_ids",
            decoder=decode_u64_list,
        )
        if active_ids:
            fail(f"{label} left unexpected active proposals")

    def phase_deploy(self) -> None:
        for slug in DEPLOYMENT_ORDER:
            deployment = self.deployments[slug]
            upload_args = {"slug": slug}
            self.transactions.execute(
                "deploy",
                "upload",
                str(deployment["wasm_hash"]),
                upload_args,
                self.build_upload(slug),
            )
        for slug in DEPLOYMENT_ORDER:
            deployment = self.deployments[slug]
            contract_id = str(deployment["contract_id"])
            deploy_args = {
                "slug": slug,
                "wasm_hash": deployment["wasm_hash"],
                "salt": deployment["salt"],
                "constructor_args": deployment["constructor_args"],
            }
            self.transactions.execute(
                "deploy",
                "deploy",
                contract_id,
                deploy_args,
                self.build_deploy(slug),
            )
            self.verify_deployment(slug)
        self.store.pass_phase(
            "deploy",
            [
                {
                    "contract_ids": {
                        slug: self.deployments[slug]["contract_id"]
                        for slug in SLUGS
                    },
                    "verified_contracts": list(SLUGS),
                }
            ],
        )

    def phase_ownership(self) -> None:
        runtime = str(self.deployments["runtime"]["contract_id"])
        governance = str(
            self.deployments["governance"]["contract_id"]
        )
        if (
            self.view(
                governance, "proxy_oracle", decoder=validate_contract
            )
            != runtime
        ):
            fail("governance points at the wrong runtime")
        owner = self.view(
            runtime, "get_owner", decoder=validate_address
        )
        if owner == self.settings.administrator:
            latest = self.latest_ledger()
            pending = self.view(
                runtime,
                "get_pending_owner",
                decoder=decode_optional_pending_owner,
            )
            transfer_entries = [
                entry
                for entry in self.store.operations()
                if entry["phase"] == "ownership"
                and entry["kind"] == "ownership_transfer"
                and entry["target"] == runtime
            ]
            if any(
                entry["status"] != "succeeded"
                for entry in transfer_entries
            ):
                fail("prior ownership transfer is not terminal-successful")
            for entry in transfer_entries:
                arguments = entry["args"]
                assert isinstance(arguments, dict)
                if arguments.get("new_owner") != governance:
                    fail("recorded ownership transfer targets another owner")
            allow_recreate_missing = False
            if pending is not None:
                if pending["address"] != governance:
                    fail("runtime has a conflicting pending owner")
                if pending["live_until_ledger"] < latest:
                    fail("runtime pending ownership is expired")
            else:
                if transfer_entries:
                    active, _, _ = self.recorded_proposal_state(
                        "ownership",
                        "runtime/accept-ownership",
                        "AcceptOwnership",
                    )
                    if active:
                        fail(
                            "expired ownership transfer still has "
                            "a pending acceptance proposal"
                        )
                    allow_recreate_missing = True
                live_until = require_u32(
                    latest + 1_000_000,
                    "ownership transfer deadline",
                    positive=True,
                )
                transfer_args = {
                    "new_owner": governance,
                    "live_until_ledger": live_until,
                }
                self.transactions.execute(
                    "ownership",
                    "ownership_transfer",
                    runtime,
                    transfer_args,
                    self.build_invoke(
                        runtime,
                        "transfer_ownership",
                        transfer_args,
                    ),
                )
                pending = self.view(
                    runtime,
                    "get_pending_owner",
                    decoder=decode_optional_pending_owner,
                )
                if pending != {
                    "address": governance,
                    "live_until_ledger": live_until,
                }:
                    fail(
                        "runtime did not persist the planned ownership "
                        "transfer"
                    )
            self.run_governance_operation(
                "ownership",
                "runtime/accept-ownership",
                "AcceptOwnership",
                allow_recreate_missing=allow_recreate_missing,
            )
        elif owner != governance:
            fail("runtime has an unexpected owner")
        if (
            self.view(
                runtime, "get_owner", decoder=validate_address
            )
            != governance
        ):
            fail("runtime ownership was not transferred to governance")
        self.store.pass_phase(
            "ownership",
            [{"runtime": runtime, "governance": governance}],
        )

    def runtime_sources(
        self,
    ) -> tuple[tuple[str, str, dict[str, str]], ...]:
        contracts = {
            "reflector": REFLECTOR,
            "redstone": REDSTONE,
            "lazer": str(self.deployments["lazer_source"]["contract_id"]),
        }
        return tuple(
            (name, contracts[name], self.settings.source_assets[name])
            for name in PROVIDER_FRESHNESS
        )

    def proxy_config(self) -> dict[str, object]:
        sources = []
        for name, contract_id, asset in self.runtime_sources():
            max_age, max_drift = provider_freshness(
                self.settings.runtime_policy, name
            )
            sources.append(
                {
                    "oracle": contract_id,
                    "asset": asset,
                    "max_age_secs": max_age,
                    "max_clock_drift_secs": max_drift,
                }
            )
        return {
            "sources": sources,
            "min_sources": 3,
            "max_cache_age_secs": self.settings.runtime_policy[
                "max_cache_age_secs"
            ],
        }

    def phase_configure(self) -> None:
        runtime = str(self.deployments["runtime"]["contract_id"])
        sanity = {}
        now = int(time.time())
        for name, contract_id, asset in self.runtime_sources():
            source = {
                "base": self.view(
                    contract_id, "base", decoder=validate_asset
                ),
                "decimals": self.view(
                    contract_id, "decimals", decoder=require_u32
                ),
                "lastprice": self.sep40_price(contract_id, asset),
            }
            if source["base"] != {"Other": "USD"}:
                fail(f"{name} source has the wrong base asset")
            decimals = source["decimals"]
            if not 0 <= decimals <= 18:
                fail(f"{name} source decimals are out of range")
            if name != "lazer":
                price = source["lastprice"]
                if price is None:
                    fail(
                        f"{name} source has no price for the configured asset"
                    )
                value = require_integer(
                    price.get("price"), f"{name} source price"
                )
                timestamp = require_integer(
                    price.get("timestamp"), f"{name} source timestamp"
                )
                max_age, max_drift = provider_freshness(
                    self.settings.runtime_policy, name
                )
                if (
                    value <= 0
                    or not now - max_age <= timestamp <= now + max_drift
                ):
                    fail(f"{name} source price is non-positive or stale")
            sanity[name] = source
        config = self.proxy_config()
        current = self.view(
            runtime,
            "get_proxy",
            {"asset": self.settings.asset},
            decoder=decode_optional_proxy_config,
        )
        if current is None:
            self.run_governance_operation(
                "configure",
                "proxy/configure",
                {"SetProxy": [self.settings.asset, config]},
            )
            current = self.view(
                runtime,
                "get_proxy",
                {"asset": self.settings.asset},
                decoder=decode_optional_proxy_config,
            )
        elif current != config:
            fail(
                "runtime proxy configuration conflicts with "
                "the requested configuration"
            )
        if current != config:
            fail(
                "runtime proxy configuration does not match "
                "the requested configuration"
            )
        self.store.pass_phase(
            "configure",
            [{"source_sanity": sanity}, {"proxy_config": current}],
        )

    def pyth_api_key(self) -> str:
        key_file = os.environ.get("PYTH_LAZER_API_KEY_FILE")
        if not key_file:
            fail("PYTH_LAZER_API_KEY_FILE is required for the push phase")
        path = Path(key_file)
        if not path.is_absolute():
            fail("PYTH_LAZER_API_KEY_FILE must be an absolute path")
        key = secure_read(path, "Pyth Lazer API key file").decode().strip()
        if not key:
            fail("Pyth Lazer API key file must not be empty")
        if any(character.isspace() for character in key):
            fail("Pyth Lazer API key must not contain whitespace")
        return key

    def lazer_request_body(self) -> dict[str, object]:
        return {
            "priceFeedIds": [self.settings.feed_id],
            "properties": [
                "price",
                "exponent",
                "feedUpdateTimestamp",
            ],
            "formats": ["leEcdsa"],
            "channel": "fixed_rate@200ms",
            "jsonBinaryEncoding": "hex",
        }

    def validate_lazer_evidence(
        self, evidence: object, payload: str
    ) -> None:
        record = require_exact_keys(
            evidence, {"request", "response"}, "Pyth Lazer evidence"
        )
        if record["request"] != self.lazer_request_body():
            fail("Pyth Lazer evidence request differs from the rehearsal")
        response = record["response"]
        if not isinstance(response, dict):
            fail("Pyth Lazer evidence response must be an object")
        encoded = response.get("leEcdsa")
        if not isinstance(encoded, dict) or encoded.get("data") != payload:
            fail(
                "Pyth Lazer evidence payload differs from "
                "the recorded transaction"
            )

    def fetch_lazer_payload(self) -> tuple[str, dict[str, object]]:
        body = self.lazer_request_body()
        request = urllib.request.Request(
            self.settings.lazer_rest,
            data=canonical_json(body),
            headers={
                "Authorization": f"Bearer {self.pyth_api_key()}",
                "Content-Type": "application/json",
                "User-Agent": HTTP_USER_AGENT,
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                raw = response.read()
        except OSError as error:
            fail(f"Pyth Lazer request failed: {error}")
        response = strict_json_bytes(raw, "Pyth Lazer response")
        if not isinstance(response, dict) or not isinstance(
            response.get("leEcdsa"), dict
        ):
            fail("Pyth Lazer response lacks leEcdsa data")
        payload = response["leEcdsa"].get("data")
        if (
            not isinstance(payload, str)
            or not payload
            or not re.fullmatch(r"[0-9a-fA-F]+", payload)
            or len(payload) % 2
        ):
            fail("Pyth Lazer payload is not non-empty even-length hexadecimal")
        evidence = {"request": body, "response": response}
        response_path = self.settings.output / "lazer_response.json"
        atomic_write(
            response_path,
            json.dumps(evidence, indent=2, sort_keys=True).encode() + b"\n",
        )
        return payload, evidence

    def phase_push(self) -> None:
        lazer = str(self.deployments["lazer_source"]["contract_id"])
        successful = [
            operation
            for operation in self.store.operations()
            if operation["phase"] == "push"
            and operation["kind"] == "push"
            and operation["status"] == "succeeded"
            and operation["target"] == lazer
        ]
        if len(successful) > 1:
            fail("multiple successful Lazer push operations recorded")
        if successful:
            operation = successful[0]
            evidence = strict_json_bytes(
                secure_read(
                    self.settings.output / "lazer_response.json",
                    "Pyth Lazer response evidence",
                ),
                "Pyth Lazer response evidence",
            )
        else:
            payload, evidence = self.fetch_lazer_payload()
            push_args = {"payload": payload}
            operation = self.transactions.execute(
                "push",
                "push",
                lazer,
                push_args,
                self.build_invoke(
                    lazer, "update_price_feeds", push_args
                ),
            )
        stored_count = self.transactions.return_value(operation)
        if (
            not isinstance(stored_count, dict)
            or type(stored_count.get("u32")) is not int
            or stored_count["u32"] <= 0
        ):
            fail("Lazer push transaction stored zero feeds")
        arguments = operation["args"]
        assert isinstance(arguments, dict)
        payload = require_string(
            arguments["payload"], "recorded Lazer payload"
        )
        self.validate_lazer_evidence(evidence, payload)
        stored = self.view(
            lazer,
            "stored_price",
            {"feed_id": self.settings.feed_id},
            decoder=decode_optional_stored_price,
        )
        lastprice = self.view(
            lazer,
            "lastprice",
            {"asset": self.settings.source_assets["lazer"]},
            decoder=decode_optional_price,
        )
        if (
            stored is None
            or require_integer(stored["mantissa"], "stored price mantissa")
            <= 0
            or lastprice is None
        ):
            fail(
                "Lazer push did not produce a positive stored "
                "and projected price"
            )
        now_us = int(time.time() * 1_000_000)
        if not (
            now_us
            - self.settings.ingest_policy["max_age_secs"] * 1_000_000
            <= stored["publish_time_us"]
            <= now_us
            + self.settings.ingest_policy[
                "max_clock_drift_secs"
            ]
            * 1_000_000
        ):
            fail("stored Lazer price timestamp is outside the ingest window")
        self.store.pass_phase(
            "push",
            [
                {"response_sha256": sha256_bytes(canonical_json(evidence))},
                {
                    "payload_sha256": sha256_bytes(
                        bytes.fromhex(payload)
                    )
                },
                {"stored_count": stored_count["u32"]},
                {"stored_price": stored},
                {"lastprice": lastprice},
            ],
        )


    def phase_refresh(self) -> None:
        runtime = str(self.deployments["runtime"]["contract_id"])
        adapter = str(self.deployments["sep40_adapter"]["contract_id"])
        batcher = str(self.deployments["batcher"]["contract_id"])
        refresh_args = {"asset": self.settings.asset}
        refresh = self.transactions.execute(
            "refresh",
            "refresh",
            runtime,
            refresh_args,
            self.build_invoke(runtime, "refresh", refresh_args),
        )
        refresh_result = self.transactions.return_value(refresh)
        accepted = require_accepted_status(
            refresh_result, "runtime refresh"
        )
        aggregated = self.view(
            runtime,
            "aggregated_latest",
            {"asset": self.settings.asset},
            decoder=decode_optional_normalized_price,
        )
        adapter_price = self.view(
            adapter,
            "lastprice",
            {"asset": self.settings.asset},
            decoder=decode_optional_price,
        )
        if aggregated is None:
            fail(
                "runtime aggregated_latest did not return "
                "a normalized price"
            )
        if aggregated != accepted:
            fail(
                "runtime Accepted result differs from aggregated_latest"
            )
        mantissa = require_integer(
            aggregated["mantissa"], "aggregated mantissa"
        )
        timestamp = require_integer(
            aggregated["timestamp"], "aggregated timestamp"
        )
        if adapter_price is None:
            fail("adapter lastprice did not return a SEP-40 price")
        expected_adapter = project_normalized_price(
            aggregated, decimals=8, resolution=1
        )
        adapter_value = require_integer(
            adapter_price["price"], "adapter price"
        )
        adapter_timestamp = require_integer(
            adapter_price["timestamp"], "adapter timestamp"
        )
        now = int(time.time())
        freshness_limits = [
            provider_freshness(self.settings.runtime_policy, name)
            for name in PROVIDER_FRESHNESS
        ]
        max_age = max(age for age, _ in freshness_limits)
        max_drift = max(drift for _, drift in freshness_limits)
        if (
            mantissa <= 0
            or adapter_price != expected_adapter
            or adapter_value <= 0
            or adapter_timestamp != timestamp
            or not now - max_age <= timestamp <= now + max_drift
        ):
            fail("runtime or adapter price is non-positive, stale, or divergent")
        refresh_many_args = {
            "oracle": runtime,
            "assets": [self.settings.asset],
        }
        refresh_many = self.transactions.execute(
            "refresh",
            "batch_refresh",
            batcher,
            refresh_many_args,
            self.build_invoke(
                batcher,
                "refresh_many",
                refresh_many_args,
            ),
        )
        refresh_many_result = self.transactions.return_value(refresh_many)
        batch_accepted = require_accepted_statuses(
            refresh_many_result, 1, "batch refresh_many"
        )
        if batch_accepted != [aggregated]:
            fail("batch refresh result differs from aggregated_latest")

        extend_assets_args = {
            "oracle": runtime,
            "assets": [self.settings.asset],
        }
        extend_assets = self.transactions.execute(
            "refresh",
            "ttl_assets",
            batcher,
            extend_assets_args,
            self.build_invoke(
                batcher,
                "extend_ttl_many",
                extend_assets_args,
            ),
        )
        extend_assets_result = self.transactions.return_value(extend_assets)
        require_true_scvals(
            extend_assets_result, 1, "batch extend_ttl_many"
        )

        extend_contracts_args = {
            "contracts": [
                str(self.deployments["governance"]["contract_id"]),
                adapter,
                str(self.deployments["lazer_source"]["contract_id"]),
            ]
        }
        extend_contracts = self.transactions.execute(
            "refresh",
            "ttl_contracts",
            batcher,
            extend_contracts_args,
            self.build_invoke(
                batcher,
                "extend_ttl_contracts",
                extend_contracts_args,
            ),
        )
        extend_contracts_result = self.transactions.return_value(
            extend_contracts
        )
        require_true_scvals(
            extend_contracts_result, 3, "batch extend_ttl_contracts"
        )
        batch_results = {
            "refresh_many": refresh_many_result,
            "extend_ttl_many": extend_assets_result,
            "extend_ttl_contracts": extend_contracts_result,
        }
        self.store.pass_phase(
            "refresh",
            [
                {"refresh": refresh_result},
                {"aggregated_latest": aggregated},
                {"adapter_lastprice": adapter_price},
                {"batcher": batch_results},
            ],
        )

    def run(self, selected_phase: str) -> None:
        requested = PHASES if selected_phase == "all" else (selected_phase,)
        for phase in requested:
            phase_results = self.store.value["phase_results"]
            assert isinstance(phase_results, dict)
            phase_index = PHASES.index(phase)
            if any(
                phase_results[prior] is None
                for prior in PHASES[:phase_index]
            ):
                fail(f"{phase} requires every preceding phase to pass")
            if phase_results[phase] is not None:
                continue
            self.verify_provider_fingerprints()
            getattr(self, f"phase_{phase}")()


def resume_or_initialize(settings: Settings, reinitialize: bool) -> CheckpointStore:
    if reinitialize:
        clear_output(settings.output)
    check_network(settings)
    check_funded(settings)
    manifest, artifacts = snapshot_release(settings)
    providers = provider_fingerprints(settings)
    existing = (
        CheckpointStore.load(settings.state_path)
        if settings.state_path.exists()
        else None
    )
    deployment_nonce = (
        str(existing.value["context"]["deployment_nonce"])  # type: ignore[index]
        if existing is not None
        else secrets.token_hex(32)
    )
    context = build_context(
        settings, manifest, artifacts, providers, deployment_nonce
    )
    if existing is not None:
        if existing.value["context"] != context:
            fail(
                "checkpoint context drift detected; "
                "use --reinitialize for a new rehearsal"
            )
        validate_deployment_plan(
            settings,
            context,
            existing.value["deployments"],
        )
        return existing
    checkpoint = initialize_checkpoint(settings, context)
    store = CheckpointStore(settings.state_path, checkpoint)
    store.first_save()
    return store


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=(*PHASES, "all"), nargs="?", default="all")
    parser.add_argument("--reinitialize", action="store_true", help="discard prior checkpoint and snapshot")
    args = parser.parse_args()
    try:
        settings = load_settings()
        with output_lock(settings.output):
            store = resume_or_initialize(settings, args.reinitialize)
            driver = Rehearsal(settings, store)
            driver.verify_provider_fingerprints()
            driver.transactions.resolve_unfinished()
            driver.verify_recorded_deployments()
            driver.run(args.phase)
        print(f"checkpoint: {settings.state_path}")
        return 0
    except (OSError, RehearsalError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
