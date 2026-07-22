#!/usr/bin/env python3
import re
import sys


PACKAGE_PREDICATE = re.compile(r"\bpackage\(([A-Za-z0-9_-]+)\)")


def main() -> None:
    filterset = sys.stdin.read()
    packages = list(dict.fromkeys(PACKAGE_PREDICATE.findall(filterset)))
    if not packages:
        raise SystemExit("error: sandbox filter contains no package predicates")

    print("\n".join(packages))


if __name__ == "__main__":
    main()
