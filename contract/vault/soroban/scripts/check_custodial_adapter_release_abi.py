#!/usr/bin/env python3
"""Validate the exact custodial-adapter reported_at release ABI."""

from __future__ import annotations

import json
import sys
from typing import Any


EXPECTED = {
    "name": "reported_at",
    "inputs": [{"name": "asset", "type_": "address"}],
    "outputs": [
        {
            "result": {
                "ok_type": {"option": {"value_type": "u64"}},
                "error_type": {"udt": {"name": "AdapterError"}},
            }
        }
    ],
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
    try:
        entries = json.load(sys.stdin)
    except (json.JSONDecodeError, OSError) as error:
        print(f"invalid Stellar interface JSON: {error}", file=sys.stderr)
        return 1
    if not isinstance(entries, list):
        print("Stellar interface JSON must be a list", file=sys.stderr)
        return 1

    matches = []
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("function_v0"), dict):
            continue
        function = entry["function_v0"]
        if function.get("name") == "reported_at":
            matches.append(normalize(function))

    if matches != [EXPECTED]:
        print("custodial-adapter ABI mismatch for reported_at", file=sys.stderr)
        print(f"expected: {json.dumps([EXPECTED], sort_keys=True)}", file=sys.stderr)
        print(f"actual:   {json.dumps(matches, sort_keys=True)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
