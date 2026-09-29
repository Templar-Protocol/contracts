#!/usr/bin/env python3
"""Validate the exact dedicated epoch-settlement vault release ABI.

Reads Stellar contract interface JSON on stdin and enforces the full deploy
surface for the epoch target in both directions:

- every expected lifecycle/version entrypoint is present exactly with the
  expected signature;
- no unexpected function exists on the artifact, which forbids any immediate
  deposit, atomic exit, pricing, fee-crystallization, or proxy-view entrypoint
  from ever reaching a deployed epoch artifact.
"""

from __future__ import annotations

import json
import sys
from typing import Any

CONTRACT_ERROR = {
    "result": {
        "ok_type": "void",
        "error_type": {"udt": {"name": "ContractError"}},
    }
}

EXPECTED: dict[str, dict[str, Any]] = {
    "version": {
        "name": "version",
        "inputs": [],
        "outputs": [{"tuple": {"value_types": ["string", "u64"]}}],
    },
    "initialize": {
        "name": "initialize",
        "inputs": [
            {"name": "curator", "type_": "address"},
            {"name": "governance", "type_": "address"},
            {"name": "asset_token", "type_": "address"},
            {"name": "share_token", "type_": "address"},
            {"name": "virtual_shares", "type_": "i128"},
            {"name": "virtual_assets", "type_": "i128"},
        ],
        "outputs": [CONTRACT_ERROR],
    },
    "initialize_with_config": {
        "name": "initialize_with_config",
        "inputs": [
            {"name": "curator", "type_": "address"},
            {"name": "governance", "type_": "address"},
            {"name": "asset_token", "type_": "address"},
            {"name": "share_token", "type_": "address"},
            {"name": "virtual_shares", "type_": "i128"},
            {"name": "virtual_assets", "type_": "i128"},
            {"name": "withdrawal_cooldown_ns", "type_": "u64"},
        ],
        "outputs": [CONTRACT_ERROR],
    },
    "initialize_with_full_config": {
        "name": "initialize_with_full_config",
        "inputs": [
            {"name": "curator", "type_": "address"},
            {"name": "governance", "type_": "address"},
            {"name": "asset_token", "type_": "address"},
            {"name": "share_token", "type_": "address"},
            {"name": "virtual_shares", "type_": "i128"},
            {"name": "virtual_assets", "type_": "i128"},
            {"name": "withdrawal_cooldown_ns", "type_": "u64"},
            {"name": "idle_resync_cooldown_ns", "type_": "u64"},
        ],
        "outputs": [CONTRACT_ERROR],
    },
    "execute": {
        "name": "execute",
        "inputs": [{"name": "payload", "type_": "bytes"}],
        "outputs": [
            {
                "result": {
                    "ok_type": "bytes",
                    "error_type": {"udt": {"name": "ContractError"}},
                }
            }
        ],
    },
    "execute_governance": {
        "name": "execute_governance",
        "inputs": [
            {"name": "caller", "type_": "address"},
            {"name": "payload", "type_": "bytes"},
        ],
        "outputs": [CONTRACT_ERROR],
    },
    "migrate": {
        "name": "migrate",
        "inputs": [{"name": "operator", "type_": "address"}],
        "outputs": [CONTRACT_ERROR],
    },
    "upgrade": {
        "name": "upgrade",
        "inputs": [
            {"name": "new_wasm_hash", "type_": {"bytes_n": {"n": 32}}},
            {"name": "operator", "type_": "address"},
        ],
        "outputs": [CONTRACT_ERROR],
    },
}

# Immediate-product entrypoints that must never appear on an epoch deploy
# artifact. The unexpected-function rule already rejects them; this list makes
# the release gate's intent explicit and stable.
FORBIDDEN = (
    "proxy_view",
    "withdraw",
    "deposit",
    "atomic_withdraw",
    "atomic_redeem",
    "allocate",
    "refresh_markets",
    "refresh_fees",
    "resync_idle_balance",
    "cancel_migration",
    "extend_ttl",
)


def normalize(function: dict[str, Any]) -> dict[str, Any]:
    return {
        "name": function.get("name"),
        "inputs": [
            {"name": item.get("name"), "type_": item.get("type_")}
            for item in function.get("inputs", [])
        ],
        "outputs": function.get("outputs", []),
    }


def main() -> int:
    try:
        entries = json.load(sys.stdin)
    except (json.JSONDecodeError, OSError) as error:
        print(f"invalid Stellar interface JSON: {error}", file=sys.stderr)
        return 1
    if not isinstance(entries, list):
        print("Stellar interface JSON must be a list", file=sys.stderr)
        return 1

    functions: dict[str, list[dict[str, Any]]] = {}
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("function_v0"), dict):
            continue
        function = entry["function_v0"]
        name = function.get("name")
        if not isinstance(name, str):
            continue
        functions.setdefault(name, []).append(normalize(function))

    valid = True
    for name, expected in EXPECTED.items():
        actual = functions.get(name, [])
        if not actual:
            valid = False
            print(f"epoch-runtime ABI is missing {name}", file=sys.stderr)
            print(f"expected: {json.dumps(expected, sort_keys=True)}", file=sys.stderr)
            continue
        if any(item != expected for item in actual):
            valid = False
            print(f"epoch-runtime ABI mismatch for {name}", file=sys.stderr)
            print(f"expected: {json.dumps(expected, sort_keys=True)}", file=sys.stderr)
            print(f"actual:   {json.dumps(actual, sort_keys=True)}", file=sys.stderr)

    for name in FORBIDDEN:
        if name in functions:
            valid = False
            print(
                f"epoch-runtime ABI exposes forbidden immediate entrypoint {name}",
                file=sys.stderr,
            )

    for name in sorted(set(functions) - set(EXPECTED)):
        valid = False
        print(f"epoch-runtime ABI exposes unexpected function {name}", file=sys.stderr)

    return 0 if valid else 1


if __name__ == "__main__":
    raise SystemExit(main())

