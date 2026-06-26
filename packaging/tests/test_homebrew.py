import importlib.util
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
HOMEBREW_SCRIPT = REPO_ROOT / "packaging" / "scripts" / "homebrew.py"


def load_homebrew_module():
    spec = importlib.util.spec_from_file_location("homebrew_script", HOMEBREW_SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_single_target_formula_requires_only_target_checksum():
    homebrew = load_homebrew_module()
    formula = {
        "name": "ve-tos-cli",
        "class": "VeTosCli",
        "description": "test formula",
        "commands": ["ve-tos-cli"],
    }
    sums = {
        "ve-storage-uni-cli-aarch64-apple-darwin.tar.gz": "abc123",
    }

    text = homebrew.formula_text(
        formula,
        "1.0.0",
        "https://github.com/volcengine/ve-storage-uni-cli",
        sums,
        target="aarch64-apple-darwin",
    )

    assert "ve-storage-uni-cli-aarch64-apple-darwin.tar.gz" in text
    assert 'sha256 "abc123"' in text
    assert "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz" not in text
    assert "ve-storage-uni-cli-aarch64-unknown-linux-gnu.tar.gz" not in text
