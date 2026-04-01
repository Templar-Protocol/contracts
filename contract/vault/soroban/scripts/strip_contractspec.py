#!/usr/bin/env python3
"""Strip contractspecv0 custom section from a Soroban WASM binary.

The contractspecv0 section is only used by client tooling (CLI, SDK bindings,
explorers). The Soroban VM only requires contractenvmetav0 at runtime.
Stripping it typically saves ~7 KiB.

Usage:
    python strip_contractspec.py --input in.wasm --output out.wasm [--section contractspecv0]
"""

import argparse
import struct
from pathlib import Path


def read_leb128(data: bytes, offset: int) -> tuple[int, int]:
    """Read an unsigned LEB128 value, return (value, new_offset)."""
    result = 0
    shift = 0
    while True:
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if (byte & 0x80) == 0:
            break
        shift += 7
    return result, offset


def encode_leb128(value: int) -> bytes:
    """Encode an unsigned integer as LEB128."""
    parts = []
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            parts.append(byte | 0x80)
        else:
            parts.append(byte)
            break
    return bytes(parts)


def strip_custom_section(wasm: bytes, section_name: str) -> bytes:
    """Remove a named custom section from a WASM binary."""
    if wasm[:4] != b"\x00asm":
        raise ValueError("not a WASM binary")

    # Keep the 8-byte header (magic + version)
    output = bytearray(wasm[:8])
    offset = 8
    stripped = 0

    while offset < len(wasm):
        section_id = wasm[offset]
        offset += 1
        section_size, offset = read_leb128(wasm, offset)
        section_start = offset
        section_end = offset + section_size

        if section_id == 0:  # custom section
            name_len, name_offset = read_leb128(wasm, section_start)
            name = wasm[name_offset : name_offset + name_len].decode("utf-8")
            if name == section_name:
                stripped += section_size + (section_start - (section_start - 1))
                offset = section_end
                continue

        # Emit section_id byte + original LEB128 size + section body
        output.append(section_id)
        output.extend(encode_leb128(section_size))
        output.extend(wasm[section_start:section_end])
        offset = section_end

    return bytes(output)


def human_bytes(value: int) -> str:
    units = ["B", "KiB", "MiB"]
    n = float(value)
    for unit in units:
        if n < 1024.0 or unit == units[-1]:
            if unit == "B":
                return f"{int(n)} {unit}"
            return f"{n:.2f} {unit}"
        n /= 1024.0
    return f"{value} B"


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Strip contractspecv0 custom section from Soroban WASM"
    )
    parser.add_argument("--input", required=True, help="Input WASM path")
    parser.add_argument("--output", required=True, help="Output WASM path")
    parser.add_argument(
        "--section",
        default="contractspecv0",
        help="Custom section name to strip (default: contractspecv0)",
    )
    args = parser.parse_args()

    input_path = Path(args.input)
    output_path = Path(args.output)

    if not input_path.exists():
        print(f"Error: {input_path} not found")
        raise SystemExit(1)

    wasm_in = input_path.read_bytes()
    wasm_out = strip_custom_section(wasm_in, args.section)

    saved = len(wasm_in) - len(wasm_out)
    output_path.write_bytes(wasm_out)

    print(f"Input:   {len(wasm_in)} bytes ({human_bytes(len(wasm_in))})")
    print(f"Output:  {len(wasm_out)} bytes ({human_bytes(len(wasm_out))})")
    if saved > 0:
        print(f"Saved:   {saved} bytes ({human_bytes(saved)})")
    else:
        print(f"No '{args.section}' section found — output identical to input")


if __name__ == "__main__":
    main()
