import importlib.util
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
PIP_SCRIPT = REPO_ROOT / "packaging" / "scripts" / "pip.py"


def load_pip_module():
    spec = importlib.util.spec_from_file_location("pip_script", PIP_SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_pip_launcher_template_lives_under_packaging_pip():
    pip = load_pip_module()

    assert pip.LAUNCHER_TEMPLATE == REPO_ROOT / "packaging" / "pip" / "launcher.py"
    assert pip.LAUNCHER_TEMPLATE.exists()


def test_pip_package_generation_uses_shared_launcher_template(tmp_path):
    pip = load_pip_module()
    binary_dir = tmp_path / "bin"
    binary_dir.mkdir()
    binary_path = binary_dir / "ve-adrive-cli"
    binary_path.write_text("#!/bin/sh\n", encoding="utf-8")

    template = pip.LAUNCHER_TEMPLATE.read_text(encoding="utf-8")
    assert "__COMMAND_ALIASES__" in template
    assert "__DEFAULT_COMMAND__" in template

    pip.generate_package(
        tmp_path / "out",
        {
            "name": "ve-adrive-cli",
            "module": "ve_adrive_cli_package",
            "commands": ["ve-adrive-cli"],
            "description": "test package",
        },
        "1.2.3",
        [binary_dir],
    )

    launcher = (
        tmp_path / "out" / "ve-adrive-cli" / "src" / "ve_adrive_cli_package" / "launcher.py"
    ).read_text(encoding="utf-8")
    assert '"ve-adrive-cli": "ve-adrive-cli"' in launcher
    assert '__DEFAULT_COMMAND__' not in launcher


def test_pip_package_generation_declares_src_package_dir(tmp_path):
    pip = load_pip_module()
    binary_dir = tmp_path / "bin"
    binary_dir.mkdir()
    (binary_dir / "ve-tos-cli").write_text("#!/bin/sh\n", encoding="utf-8")

    pip.generate_package(
        tmp_path / "out",
        {
            "name": "ve-tos-cli",
            "module": "ve_tos_cli_package",
            "commands": ["ve-tos-cli"],
            "description": "test package",
        },
        "1.2.3",
        [binary_dir],
    )

    pyproject = (tmp_path / "out" / "ve-tos-cli" / "pyproject.toml").read_text(
        encoding="utf-8"
    )

    assert "[tool.setuptools.package-dir]" in pyproject
    assert '"" = "src"' in pyproject


def test_pip_setup_forces_py3_none_platform_wheel_tags(tmp_path):
    pip = load_pip_module()
    binary_dir = tmp_path / "bin"
    binary_dir.mkdir()
    (binary_dir / "ve-tos-cli").write_text("#!/bin/sh\n", encoding="utf-8")

    pip.generate_package(
        tmp_path / "out",
        {
            "name": "ve-tos-cli",
            "module": "ve_tos_cli_package",
            "commands": ["ve-tos-cli"],
            "description": "test package",
        },
        "1.2.3",
        [binary_dir],
    )

    setup_py = (tmp_path / "out" / "ve-tos-cli" / "setup.py").read_text(encoding="utf-8")

    assert 'return (python_tag, "none", platform_tag)' in setup_py
    assert 'cmdclass={"bdist_wheel": PlatformWheel}' in setup_py
