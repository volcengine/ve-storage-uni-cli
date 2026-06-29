import subprocess
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
PUBLIC_REPOSITORY = "https://github.com/volcengine/ve-storage-uni-cli"

ROOT_PACKAGE = REPO_ROOT / "Cargo.toml"
CORE_PACKAGE_INCLUDES = {
    REPO_ROOT / "crates" / "tos-core" / "Cargo.toml": [
        "/Cargo.toml",
        "/build.rs",
        "/src/**",
    ],
    REPO_ROOT / "crates" / "tos" / "Cargo.toml": [
        "/Cargo.toml",
        "/src/**",
    ],
    REPO_ROOT / "crates" / "toscli" / "Cargo.toml": [
        "/Cargo.toml",
        "/src/**",
    ],
    REPO_ROOT / "crates" / "adrive" / "Cargo.toml": [
        "/Cargo.toml",
        "/src/**",
    ],
}
ENTRY_PACKAGE_INCLUDES = {
    REPO_ROOT / "packaging" / "cargo" / "ve-tos-cli" / "Cargo.toml",
    REPO_ROOT / "packaging" / "cargo" / "tos-cli" / "Cargo.toml",
    REPO_ROOT / "packaging" / "cargo" / "ve-adrive-cli" / "Cargo.toml",
}


def manifest(path: Path) -> dict[str, object]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def cargo_package_list(package_name: str) -> list[str]:
    result = subprocess.run(
        ["cargo", "package", "-p", package_name, "--list", "--allow-dirty"],
        cwd=REPO_ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return result.stdout.splitlines()


def test_root_crate_include_excludes_repository_only_files():
    package = manifest(ROOT_PACKAGE)["package"]

    assert package["include"] == [
        "/Cargo.toml",
        "/Cargo.lock",
        "/README.md",
        "/LICENSE",
        "/src/**",
    ]


def test_publishable_core_crates_have_minimal_include_sets():
    for manifest_path, include in CORE_PACKAGE_INCLUDES.items():
        package = manifest(manifest_path)["package"]

        assert package["include"] == include


def test_public_entry_crates_have_minimal_include_sets():
    for manifest_path in ENTRY_PACKAGE_INCLUDES:
        package = manifest(manifest_path)["package"]

        assert package["include"] == [
            "/Cargo.toml",
            "/Cargo.lock",
            "/src/**",
        ]


def test_publishable_crates_point_to_public_repository():
    manifest_paths = [
        ROOT_PACKAGE,
        *CORE_PACKAGE_INCLUDES,
        *ENTRY_PACKAGE_INCLUDES,
    ]

    for manifest_path in manifest_paths:
        package = manifest(manifest_path)["package"]

        assert package["homepage"] == PUBLIC_REPOSITORY
        assert package["repository"] == PUBLIC_REPOSITORY


def test_root_cargo_package_list_omits_repository_only_files():
    packaged_files = cargo_package_list("ve-storage-uni-cli")

    forbidden_prefixes = (
        ".codebase/",
        "packaging/",
        "scripts/e2e/",
        "skills/",
        "tests/",
    )
    for packaged_file in packaged_files:
        assert not packaged_file.startswith(forbidden_prefixes), packaged_file
