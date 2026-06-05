#!/usr/bin/env python3
"""Encode and decode compact Templar Soroban vault ABI payloads."""

from __future__ import annotations

import argparse
import json
import sys


def push_u8(out: bytearray, value: int) -> None:
    out.append(value & 0xFF)


def push_u32(out: bytearray, value: int) -> None:
    out.extend(value.to_bytes(4, "little", signed=False))


def push_u64(out: bytearray, value: int) -> None:
    out.extend(value.to_bytes(8, "little", signed=False))


def push_u128(out: bytearray, value: int) -> None:
    out.extend(value.to_bytes(16, "little", signed=False))


def push_i128(out: bytearray, value: int) -> None:
    out.extend(value.to_bytes(16, "little", signed=True))


def push_string(out: bytearray, value: str) -> None:
    encoded = value.encode("utf-8")
    push_u32(out, len(encoded))
    out.extend(encoded)


def push_u32_vec(out: bytearray, values: list[int]) -> None:
    push_u32(out, len(values))
    for value in values:
        push_u32(out, value)


def read_exact(data: bytes, cursor: int, length: int) -> tuple[bytes, int]:
    end = cursor + length
    if end > len(data):
        raise ValueError("truncated payload")
    return data[cursor:end], end


def read_u8(data: bytes, cursor: int) -> tuple[int, int]:
    raw, cursor = read_exact(data, cursor, 1)
    return raw[0], cursor


def read_u32(data: bytes, cursor: int) -> tuple[int, int]:
    raw, cursor = read_exact(data, cursor, 4)
    return int.from_bytes(raw, "little", signed=False), cursor


def read_u64(data: bytes, cursor: int) -> tuple[int, int]:
    raw, cursor = read_exact(data, cursor, 8)
    return int.from_bytes(raw, "little", signed=False), cursor


def read_u128(data: bytes, cursor: int) -> tuple[int, int]:
    raw, cursor = read_exact(data, cursor, 16)
    return int.from_bytes(raw, "little", signed=False), cursor


def read_i128(data: bytes, cursor: int) -> tuple[int, int]:
    raw, cursor = read_exact(data, cursor, 16)
    return int.from_bytes(raw, "little", signed=True), cursor


def parse_u32_json(value: str) -> list[int]:
    parsed = json.loads(value)
    if not isinstance(parsed, list):
        raise ValueError("expected a JSON array of u32 values")
    values: list[int] = []
    for item in parsed:
        if type(item) is not int or not 0 <= item <= 0xFFFFFFFF:
            raise ValueError("expected a JSON array of u32 values")
        values.append(item)
    return values


def encode_vault(args: argparse.Namespace) -> str:
    out = bytearray()
    command = args.command
    if command == "deposit-with-min":
        push_u8(out, 0)
        push_string(out, args.owner)
        push_string(out, args.receiver)
        push_i128(out, int(args.assets))
        push_i128(out, int(args.min_shares_out))
    elif command == "request-withdraw":
        push_u8(out, 1)
        push_string(out, args.owner)
        push_string(out, args.receiver)
        push_i128(out, int(args.shares))
        push_i128(out, int(args.min_assets_out))
    elif command == "execute-withdraw":
        push_u8(out, 2)
        push_string(out, args.caller)
    elif command == "allocate":
        push_u8(out, 3)
        push_string(out, args.caller)
        push_u32(out, int(args.market))
        push_i128(out, int(args.amount))
        push_u8(out, 1 if args.supply else 0)
    elif command == "refresh-markets":
        push_u8(out, 4)
        push_string(out, args.caller)
        push_u32_vec(out, parse_u32_json(args.markets))
    elif command == "refresh-fees":
        push_u8(out, 5)
    elif command == "atomic-withdraw":
        push_u8(out, 6)
        push_string(out, args.owner)
        push_string(out, args.receiver)
        push_string(out, args.operator)
        push_i128(out, int(args.assets))
        push_i128(out, int(args.max_shares_burned))
    elif command == "atomic-redeem":
        push_u8(out, 7)
        push_string(out, args.owner)
        push_string(out, args.receiver)
        push_string(out, args.operator)
        push_i128(out, int(args.shares))
        push_i128(out, int(args.min_assets_out))
    elif command == "resync-idle-balance":
        push_u8(out, 8)
    elif command == "cancel-migration":
        push_u8(out, 9)
        push_string(out, args.caller)
    elif command == "extend-ttl":
        push_u8(out, 10)
    elif command == "abort-withdrawing":
        push_u8(out, 11)
        push_string(out, args.caller)
        push_u64(out, int(args.op_id))
    else:
        raise ValueError(f"unknown vault command: {command}")
    return out.hex()


def decode_result(hex_payload: str) -> dict[str, object]:
    data = bytes.fromhex(hex_payload.removeprefix("0x"))
    tag, cursor = read_u8(data, 0)
    if tag == 0:
        result: dict[str, object] = {"kind": "unit"}
    elif tag == 1:
        value, cursor = read_i128(data, cursor)
        result = {"kind": "i128", "value": value}
    elif tag == 2:
        value, cursor = read_u64(data, cursor)
        result = {"kind": "u64", "value": value}
    elif tag == 3:
        before, cursor = read_u32(data, cursor)
        after, cursor = read_u32(data, cursor)
        assets, cursor = read_u128(data, cursor)
        events, cursor = read_u32(data, cursor)
        result = {
            "kind": "execute_withdraw_status",
            "op_state_before": before,
            "op_state_after": after,
            "assets_transferred": assets,
            "events_emitted": events,
        }
    else:
        raise ValueError(f"invalid result tag: {tag}")
    if cursor != len(data):
        raise ValueError("trailing bytes in result")
    return result


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="mode", required=True)

    vault = subcommands.add_parser("vault", help="encode a VaultCommand")
    vault_sub = vault.add_subparsers(dest="command", required=True)

    deposit = vault_sub.add_parser("deposit-with-min")
    deposit.add_argument("--owner", required=True)
    deposit.add_argument("--receiver", required=True)
    deposit.add_argument("--assets", required=True)
    deposit.add_argument("--min-shares-out", required=True)

    request = vault_sub.add_parser("request-withdraw")
    request.add_argument("--owner", required=True)
    request.add_argument("--receiver", required=True)
    request.add_argument("--shares", required=True)
    request.add_argument("--min-assets-out", required=True)

    execute = vault_sub.add_parser("execute-withdraw")
    execute.add_argument("--caller", required=True)

    abort = vault_sub.add_parser("abort-withdrawing")
    abort.add_argument("--caller", required=True)
    abort.add_argument("--op-id", required=True)

    allocate = vault_sub.add_parser("allocate")
    allocate.add_argument("--caller", required=True)
    allocate.add_argument("--market", required=True)
    allocate.add_argument("--amount", required=True)
    allocate.add_argument("--supply", action=argparse.BooleanOptionalAction, default=True)

    refresh = vault_sub.add_parser("refresh-markets")
    refresh.add_argument("--caller", required=True)
    refresh.add_argument("--markets", required=True, help="JSON array of u32 market ids")

    vault_sub.add_parser("refresh-fees")

    atomic_withdraw = vault_sub.add_parser("atomic-withdraw")
    atomic_withdraw.add_argument("--owner", required=True)
    atomic_withdraw.add_argument("--receiver", required=True)
    atomic_withdraw.add_argument("--operator", required=True)
    atomic_withdraw.add_argument("--assets", required=True)
    atomic_withdraw.add_argument("--max-shares-burned", required=True)

    atomic_redeem = vault_sub.add_parser("atomic-redeem")
    atomic_redeem.add_argument("--owner", required=True)
    atomic_redeem.add_argument("--receiver", required=True)
    atomic_redeem.add_argument("--operator", required=True)
    atomic_redeem.add_argument("--shares", required=True)
    atomic_redeem.add_argument("--min-assets-out", required=True)

    vault_sub.add_parser("resync-idle-balance")

    cancel = vault_sub.add_parser("cancel-migration")
    cancel.add_argument("--caller", required=True)

    vault_sub.add_parser("extend-ttl")

    result = subcommands.add_parser("result", help="decode a VaultCommandResult hex payload")
    result.add_argument("hex")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        if args.mode == "vault":
            print(encode_vault(args))
        elif args.mode == "result":
            print(json.dumps(decode_result(args.hex), sort_keys=True))
        else:
            raise ValueError(f"unknown mode: {args.mode}")
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
