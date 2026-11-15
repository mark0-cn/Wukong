#!/usr/bin/env python3
"""Stable worker CLI entry; signed source trees must never gain bytecode files."""

import sys

sys.dont_write_bytecode = True
from wukong_worker.cli import main

if __name__ == "__main__":
    main()
