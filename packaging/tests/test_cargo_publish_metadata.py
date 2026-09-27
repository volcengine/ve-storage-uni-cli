import json
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


def test_core_package_contains_public_zti_sources():
    packaged_files = cargo_package_list("tos-core")
    for source in (
        "src/infra/zti_agent.rs",
        "src/infra/zti_credentials.rs",
        "src/infra/zti_source.rs",
    ):
        assert source in packaged_files


def test_dedicated_entry_locks_resolve_public_zti_dependencies_offline():
    for manifest_path in ENTRY_PACKAGE_INCLUDES:
        result = subprocess.run(
            [
                "cargo", "metadata", "--manifest-path", str(manifest_path),
                "--locked", "--offline", "--format-version", "1",
            ],
            cwd=REPO_ROOT,
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        )
        packages = json.loads(result.stdout)["packages"]
        names = {package["name"] for package in packages}
        assert {"tos-core", "tonic", "prost", "tower"} <= names
        # [Review Fix #3] A path package could hide a private dependency from
        # the registry-source check, and an unused lock entry still ships.
        assert all(
            not Path(package["manifest_path"]).resolve().is_relative_to(REPO_ROOT / "internal")
            for package in packages
        )
        assert all(
            package["source"] is None
            or package["source"] == "registry+https://github.com/rust-lang/crates.io-index"
            for package in packages
        )
        locked_packages = tomllib.loads(
            manifest_path.with_name("Cargo.lock").read_text(encoding="utf-8")
        )["package"]
        assert all(
            package.get("source", "registry+https://github.com/rust-lang/crates.io-index")
            == "registry+https://github.com/rust-lang/crates.io-index"
            for package in locked_packages
        )


def test_dedicated_entry_packages_include_the_locked_binary_source():
    for manifest_path in ENTRY_PACKAGE_INCLUDES:
        result = subprocess.run(
            [
                "cargo", "package", "--manifest-path", str(manifest_path),
                "--list", "--allow-dirty", "--locked", "--offline",
            ],
            cwd=REPO_ROOT,
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        )
        packaged_files = result.stdout.splitlines()
        assert "Cargo.lock" in packaged_files
        assert "src/main.rs" in packaged_files
