#!/usr/bin/env python3
"""Run one selected local Forge test with traces; use the Bash tool deadline."""
import argparse
import shutil
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default=".")
    parser.add_argument("--match-contract", required=True)
    parser.add_argument("--match-test", required=True)
    parser.add_argument("--forge", default="forge")
    args = parser.parse_args()
    if not args.match_contract.strip() or not args.match_test.strip():
        parser.error("contract and test filters must be nonempty")
    forge = shutil.which(args.forge)
    if forge is None:
        parser.error("Forge not found; select an installed executable with --forge")
    root = Path(args.root).resolve(strict=True)
    if not root.is_dir():
        parser.error("--root must be a directory")
    version = subprocess.run([forge, "--version"], cwd=root, check=False)
    if version.returncode:
        return version.returncode
    return subprocess.run([
        forge, "test", "--root", str(root), "--match-contract", args.match_contract,
        "--match-test", args.match_test, "-vvvv",
    ], cwd=root, check=False).returncode


if __name__ == "__main__":
    sys.exit(main())
