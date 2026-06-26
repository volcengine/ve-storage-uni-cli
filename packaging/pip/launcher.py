"""Console-script launcher for packaged Volcengine storage CLI binaries."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


COMMAND_ALIASES = {
__COMMAND_ALIASES__
}


def _binary_path(command_name: str) -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return Path(__file__).with_name("bin") / f"{command_name}{suffix}"


def run(command_name: str) -> int:
    binary_path = _binary_path(command_name)
    if not binary_path.exists():
        print(f"{command_name} binary is missing from the installed wheel", file=sys.stderr)
        return 1

    argv = [str(binary_path), *sys.argv[1:]]
    if os.name == "nt":
        return subprocess.run(argv, check=False).returncode

    os.execv(str(binary_path), argv)
    return 1


def main() -> int:
    entry_name = Path(sys.argv[0]).name
    command_name = COMMAND_ALIASES.get(entry_name, "__DEFAULT_COMMAND__")
    return run(command_name)
