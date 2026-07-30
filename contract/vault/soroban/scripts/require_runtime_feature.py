#!/usr/bin/env python3
"""Require a feature bit in a Stellar runtime version response read from stdin."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


def parse_response(payload: Any) -> tuple[str, int]:
    if not isinstance(payload, list) or len(payload) != 2:
        raise ValueError("expected [version, feature_flags]")
    version, feature_flags = payload
    if not isinstance(version, str):
        raise ValueError("version must be a string")
    if isinstance(feature_flags, bool) or not isinstance(feature_flags, int):
        raise ValueError("feature_flags must be an integer")
    if not 0 <= feature_flags <= (1 << 64) - 1:
        raise ValueError("feature_flags must fit u64")
    return version, feature_flags


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--feature-bit", required=True, type=lambda value: int(value, 0))
    parser.add_argument("--feature-name", required=True)
    parser.add_argument("--contract", required=True)
    args = parser.parse_args()

    if args.feature_bit <= 0 or args.feature_bit > (1 << 64) - 1:
        parser.error("--feature-bit must be a nonzero u64 value")

    try:
        version, feature_flags = parse_response(json.load(sys.stdin))
    except (json.JSONDecodeError, OSError, ValueError) as error:
        print(
            f"cannot use {args.contract}: invalid runtime version response: {error}",
            file=sys.stderr,
        )
        return 1

    if feature_flags & args.feature_bit == 0:
        print(
            f"cannot use {args.contract}: version {version} does not advertise "
            f"{args.feature_name} capability {args.feature_bit:#x} "
            f"(feature mask {feature_flags:#x})",
            file=sys.stderr,
        )
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
