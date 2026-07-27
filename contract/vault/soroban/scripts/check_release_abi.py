#!/usr/bin/env python3
"""Validate the exact runtime and curator-proxy version interfaces."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


CONTRACT_ERROR = {
    "result": {
        "ok_type": "void",
        "error_type": {"udt": {"name": "ContractError"}},
    }
}

EXPECTED: dict[str, dict[str, dict[str, Any]]] = {
    "runtime": {
        "version": {
            "name": "version",
            "inputs": [],
            "outputs": [{"tuple": {"value_types": ["string", "u64"]}}],
        }
    },
    "curator-proxy": {
        "__constructor": {
            "name": "__constructor",
            "inputs": [
                {"name": "initialization_authority", "type_": "address"},
            ],
            "outputs": [],
        },
        "initialize": {
            "name": "initialize",
            "inputs": [
                {"name": "vault_address", "type_": "address"},
                {"name": "governance_address", "type_": "address"},
            ],
            "outputs": [CONTRACT_ERROR],
        },
        "initialize_legacy_v1": {
            "name": "initialize_legacy_v1",
            "inputs": [
                {"name": "vault_address", "type_": "address"},
                {"name": "governance_address", "type_": "address"},
                {
                    "name": "legacy_v1_wasm_hash",
                    "type_": {"bytes_n": {"n": 32}},
                },
            ],
            "outputs": [CONTRACT_ERROR],
        },
        "vault_version": {
            "name": "vault_version",
            "inputs": [],
            "outputs": [
                {
                    "result": {
                        "ok_type": {
                            "tuple": {"value_types": ["string", "u64"]}
                        },
                        "error_type": {"udt": {"name": "ContractError"}},
                    }
                }
            ],
        },
    },
}


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
    parser = argparse.ArgumentParser()
    parser.add_argument("--contract", choices=EXPECTED, required=True)
    args = parser.parse_args()

    try:
        entries = json.load(sys.stdin)
    except (json.JSONDecodeError, OSError) as error:
        print(f"invalid Stellar interface JSON: {error}", file=sys.stderr)
        return 1
    if not isinstance(entries, list):
        print("Stellar interface JSON must be a list", file=sys.stderr)
        return 1

    functions: dict[str, dict[str, Any]] = {}
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("function_v0"), dict):
            continue
        function = entry["function_v0"]
        name = function.get("name")
        if not isinstance(name, str):
            continue
        if name in functions:
            print(f"duplicate function in contract interface: {name}", file=sys.stderr)
            return 1
        functions[name] = normalize(function)

    valid = True
    for name, expected in EXPECTED[args.contract].items():
        actual = functions.get(name)
        if actual != expected:
            valid = False
            print(f"{args.contract} ABI mismatch for {name}", file=sys.stderr)
            print(f"expected: {json.dumps(expected, sort_keys=True)}", file=sys.stderr)
            print(f"actual:   {json.dumps(actual, sort_keys=True)}", file=sys.stderr)

    return 0 if valid else 1


if __name__ == "__main__":
    raise SystemExit(main())
