#!/usr/bin/env python3
"""Validate an adapter's exact release ABI from `stellar contract info interface --output json`.

Reads the interface JSON on stdin. With no argument, asserts the custodial-adapter `reported_at`
signature. With `--spec <path-or-inline-json>`, asserts the spec instead:

    {
      "label": "carry-adapter",
      "expected": [ <normalised function dict>, ... ],
      "forbidden_exports": [ "rescue", ... ]
    }

A normalised function dict is `{"name", "inputs": [{"name", "type_"}], "outputs": [...]}`. An
expected entry may add `"unwrap_result": true`: the actual `outputs` are then reduced from
`[{"result": {"ok_type": T, ...}}]` to `[T]` (`void` -> `[]`) before comparing, so the error enum
is intentionally not pinned. Each expected name must match exactly one exported function.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from typing import Any


DEFAULT_SPEC = {
    "label": "custodial-adapter",
    "expected": [
        {
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
    ],
    "forbidden_exports": [],
}


def normalize(function: dict[str, Any], unwrap_result: bool = False) -> dict[str, Any]:
    outputs = function.get("outputs", [])
    if unwrap_result and len(outputs) == 1 and isinstance(outputs[0], dict) and "result" in outputs[0]:
        ok_type = outputs[0]["result"].get("ok_type")
        outputs = [] if ok_type == "void" else [ok_type]
    return {
        "name": function.get("name"),
        "inputs": [
            {"name": item.get("name"), "type_": item.get("type_")}
            for item in function.get("inputs", [])
        ],
        "outputs": outputs,
    }


def load_spec(arg: str | None) -> dict[str, Any]:
    if arg is None:
        return DEFAULT_SPEC
    text = open(arg, encoding="utf-8").read() if os.path.isfile(arg) else arg
    spec = json.loads(text)
    if not isinstance(spec, dict) or not isinstance(spec.get("expected"), list):
        raise ValueError("spec must be an object with an `expected` list")
    return {
        "label": spec.get("label", "adapter"),
        "expected": spec["expected"],
        "forbidden_exports": spec.get("forbidden_exports", []),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--spec", help="path to, or inline JSON of, the ABI spec")
    args = parser.parse_args()
    try:
        spec = load_spec(args.spec)
    except (json.JSONDecodeError, OSError, ValueError) as error:
        print(f"invalid ABI spec: {error}", file=sys.stderr)
        return 1

    try:
        entries = json.load(sys.stdin)
    except (json.JSONDecodeError, OSError) as error:
        print(f"invalid Stellar interface JSON: {error}", file=sys.stderr)
        return 1
    if not isinstance(entries, list):
        print("Stellar interface JSON must be a list", file=sys.stderr)
        return 1

    functions = [
        entry["function_v0"]
        for entry in entries
        if isinstance(entry, dict) and isinstance(entry.get("function_v0"), dict)
    ]
    status = 0
    for expected in spec["expected"]:
        want = {k: v for k, v in expected.items() if k != "unwrap_result"}
        matches = [
            normalize(f, bool(expected.get("unwrap_result")))
            for f in functions
            if f.get("name") == want["name"]
        ]
        if matches != [want]:
            print(f"{spec['label']} ABI mismatch for {want['name']}", file=sys.stderr)
            print(f"expected: {json.dumps([want], sort_keys=True)}", file=sys.stderr)
            print(f"actual:   {json.dumps(matches, sort_keys=True)}", file=sys.stderr)
            status = 1

    exported = {f.get("name") for f in functions}
    for name in spec["forbidden_exports"]:
        if name in exported:
            print(f"{spec['label']} exports forbidden function `{name}`", file=sys.stderr)
            status = 1
    return status


if __name__ == "__main__":
    raise SystemExit(main())
