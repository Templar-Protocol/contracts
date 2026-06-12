#!/usr/bin/env python3
import argparse
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description="Enforce WASM size budget")
    parser.add_argument("--wasm", required=True, help="Path to wasm file")
    parser.add_argument(
        "--max-bytes",
        type=int,
        required=True,
        help="Maximum allowed size in bytes",
    )
    args = parser.parse_args()

    wasm_path = Path(args.wasm)
    if not wasm_path.exists():
        raise SystemExit(f"error: missing wasm artifact: {wasm_path}")

    size = wasm_path.stat().st_size
    max_bytes = args.max_bytes
    size_kib = size / 1024
    max_kib = max_bytes / 1024

    print(
        f"WASM budget check: {size} bytes ({size_kib:.2f} KiB) <= {max_bytes} bytes ({max_kib:.2f} KiB)"
    )

    if size > max_bytes:
        raise SystemExit(
            f"error: wasm size budget exceeded by {size - max_bytes} bytes ({size_kib:.2f} KiB > {max_kib:.2f} KiB)"
        )


if __name__ == "__main__":
    main()
