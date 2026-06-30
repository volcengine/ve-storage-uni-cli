#!/usr/bin/env python3
# Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
# http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""Generate PyPI wheel source trees for the public CLI install names."""

from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
LAUNCHER_TEMPLATE = ROOT / "packaging" / "pip" / "launcher.py"


SETUP_PY = r'''"""Build a platform-specific wheel because this package contains native binaries."""

from setuptools import Distribution, setup
from wheel.bdist_wheel import bdist_wheel


class BinaryDistribution(Distribution):
    def has_ext_modules(self):
        return True


class PlatformWheel(bdist_wheel):
    def finalize_options(self):
        super().finalize_options()
        # [Review Fix #PipPlatformWheel] The wheel contains platform-specific
        # CLI binaries, but no Python ABI-specific extension module.
        self.root_is_pure = False

    def get_tag(self):
        _python_tag, _abi_tag, platform_tag = super().get_tag()
        python_tag = self.python_tag or "py3"
        return (python_tag, "none", platform_tag)


setup(distclass=BinaryDistribution, cmdclass={"bdist_wheel": PlatformWheel})
'''


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Python package version")
    parser.add_argument(
        "--binary-dir",
        required=True,
        action="append",
        type=Path,
        help="Directory containing built CLI binaries for the wheel platform; can be repeated",
    )
    parser.add_argument(
        "--config",
        default=Path("packaging/pip/packages.json"),
        type=Path,
        help="pip package definition file",
    )
    parser.add_argument(
        "--out-dir",
        default=Path("dist/pip"),
        type=Path,
        help="Output directory for generated Python package trees",
    )
    return parser.parse_args()


def write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def command_aliases(commands: list[str]) -> str:
    return "\n".join(f'    "{command}": "{command}",' for command in commands)


def entry_points(module_name: str, commands: list[str]) -> dict[str, str]:
    return {command: f"{module_name}.launcher:main" for command in commands}


def find_binary(binary_dirs: list[Path], command: str) -> Path:
    for binary_dir in binary_dirs:
        source = binary_dir / command
        if source.exists():
            return source

        windows_source = binary_dir / f"{command}.exe"
        if windows_source.exists():
            return windows_source

    searched = ", ".join(str(binary_dir) for binary_dir in binary_dirs)
    raise SystemExit(f"missing built binary {command}; searched: {searched}")


def copy_binaries(binary_dirs: list[Path], package_bin_dir: Path, commands: list[str]) -> None:
    package_bin_dir.mkdir(parents=True, exist_ok=True)
    for command in commands:
        source = find_binary(binary_dirs, command)
        dest = package_bin_dir / source.name
        shutil.copy2(source, dest)
        dest.chmod(0o755)


def generate_package(
    out_dir: Path,
    package: dict[str, object],
    version: str,
    binary_dirs: list[Path],
) -> None:
    package_name = str(package["name"])
    module_name = str(package["module"])
    commands = list(package["commands"])
    package_dir = out_dir / package_name
    if package_dir.exists():
        shutil.rmtree(package_dir)
    src_dir = package_dir / "src" / module_name
    bin_dir = src_dir / "bin"
    src_dir.mkdir(parents=True)

    pyproject = {
        "build-system": {
            "requires": ["setuptools>=68", "wheel"],
            "build-backend": "setuptools.build_meta",
        },
        "project": {
            "name": package_name,
            "version": version,
            "description": package["description"],
            "readme": "README.md",
            "requires-python": ">=3.8",
            "license": {"text": "Apache-2.0"},
            "scripts": entry_points(module_name, commands),
        },
        "tool": {
            "setuptools": {
                "packages": [module_name],
                # [Review Fix #PipSrcLayout] setuptools otherwise looks for
                # the package at the project root instead of under src/.
                "package-dir": {"": "src"},
            },
            "setuptools.package-data": {module_name: ["bin/*"]},
        },
    }
    write_text(package_dir / "pyproject.toml", toml_text(pyproject))
    write_text(
        package_dir / "README.md",
        f"# {package_name}\n\nThin PyPI package for Volcengine storage CLI binaries.\n",
    )
    write_text(package_dir / "setup.py", SETUP_PY)
    write_text(src_dir / "__init__.py", '"""Packaged Volcengine storage CLI launcher."""\n')
    launcher = LAUNCHER_TEMPLATE.read_text(encoding="utf-8")
    write_text(
        src_dir / "launcher.py",
        launcher.replace("__COMMAND_ALIASES__", command_aliases(commands)).replace(
            "__DEFAULT_COMMAND__", commands[0]
        ),
    )
    copy_binaries(binary_dirs, bin_dir, commands)


def toml_text(value: dict[str, object]) -> str:
    lines: list[str] = []

    def emit_table(prefix: str, table: dict[str, object]) -> None:
        lines.append(f"[{prefix}]")
        for key, item in table.items():
            if isinstance(item, dict):
                continue
            lines.append(f"{toml_key(key)} = {toml_value(item)}")
        lines.append("")
        for key, item in table.items():
            if isinstance(item, dict):
                emit_table(f"{prefix}.{key}", item)

    for key, item in value.items():
        if not isinstance(item, dict):
            raise TypeError(f"top-level value for {key} must be a table")
        emit_table(key, item)
    return "\n".join(lines)


def toml_key(key: str) -> str:
    if key:
        return key
    return json.dumps(key)


def toml_value(value: object) -> str:
    if isinstance(value, str):
        return json.dumps(value)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    if isinstance(value, dict):
        pairs = ", ".join(f"{toml_key(key)} = {toml_value(item)}" for key, item in value.items())
        return "{ " + pairs + " }"
    raise TypeError(f"unsupported TOML value: {value!r}")


def main() -> None:
    args = parse_args()
    config = json.loads(args.config.read_text(encoding="utf-8"))
    for package in config["packages"]:
        generate_package(args.out_dir, package, args.version, args.binary_dir)


if __name__ == "__main__":
    main()
