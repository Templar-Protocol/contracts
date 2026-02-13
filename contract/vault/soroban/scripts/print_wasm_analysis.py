#!/usr/bin/env python3
import argparse
import re
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


def print_report(path: Path, limit: int) -> None:
    print(f"\n=== {path} ===")
    if not path.exists():
        print("missing (run: just wasm-analyze)")
        return

    count = 0
    for raw in path.read_text(encoding="utf-8", errors="ignore").splitlines():
        line = raw.rstrip()
        if not line or line.startswith("─"):
            continue
        if "Bytes" in line and "Item" in line:
            print(line)
            continue

        parts = [p.strip() for p in line.split("┊")]
        if len(parts) >= 3:
            m = re.match(r"^(\d+)$", parts[0])
            if m:
                size = int(m.group(1))
                item = parts[-1]
                print(f"{parts[0]:>12} ({human_bytes(size):>10})  {item}")
                count += 1
                if limit > 0 and count >= limit:
                    print("... (truncated)")
                    return
                continue

        print(line)


def main() -> None:
    parser = argparse.ArgumentParser(description="Pretty-print wasm analysis reports")
    parser.add_argument(
        "--dir", required=True, help="Directory with top/dominators/monos reports"
    )
    parser.add_argument(
        "--report", choices=["top", "dominators", "monos", "all"], default="all"
    )
    parser.add_argument(
        "--lines", type=int, default=120, help="Max entries per report (0 = unlimited)"
    )
    args = parser.parse_args()

    base = Path(args.dir)
    files = {
        "top": ["top.txt"],
        "dominators": ["dominators.txt"],
        "monos": ["monos.txt"],
        "all": ["top.txt", "dominators.txt", "monos.txt"],
    }[args.report]

    for name in files:
        print_report(base / name, args.lines)


if __name__ == "__main__":
    main()
