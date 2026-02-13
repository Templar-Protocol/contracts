#!/usr/bin/env python3
import argparse
from pathlib import Path


def human_bytes(value: int) -> str:
    units = ["B", "KiB", "MiB", "GiB", "TiB"]
    n = float(value)
    for unit in units:
        if n < 1024.0 or unit == units[-1]:
            if unit == "B":
                return f"{int(n)} {unit}"
            return f"{n:.2f} {unit}"
        n /= 1024.0
    return f"{value} B"


def read_size(path: Path) -> int:
    if not path.exists():
        raise FileNotFoundError(f"missing file: {path}")
    return path.stat().st_size


def main() -> None:
    parser = argparse.ArgumentParser(description="Print WASM size summary")
    parser.add_argument("--wasm", required=True, help="Path to release wasm")
    parser.add_argument("--optimized", required=True, help="Path to optimized wasm")
    args = parser.parse_args()

    wasm_path = Path(args.wasm)
    optimized_path = Path(args.optimized)

    wasm_size = read_size(wasm_path)
    optimized_size = read_size(optimized_path)
    delta = wasm_size - optimized_size
    pct = (delta / wasm_size * 100.0) if wasm_size else 0.0

    print("WASM size summary:")
    print(f"  release:   {wasm_size} bytes ({human_bytes(wasm_size)})")
    print(f"  optimized: {optimized_size} bytes ({human_bytes(optimized_size)})")
    print(f"  saved:     {delta} bytes ({human_bytes(delta)}; {pct:.2f}%)")


if __name__ == "__main__":
    main()
