from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
PUBLIC_SKILLS = {
    "ve-tos-cli": {
        "command": "ve-tos-cli",
        "cargo": "cargo install ve-tos-cli",
        "npm": "npm install -g ve-tos-cli",
        "pip": "pip install ve-tos-cli",
        "brew": "brew install ve-tos-cli",
        "winget": "winget install ve-tos-cli",
    },
    "tos-cli": {
        "command": "tos-cli",
        "cargo": "cargo install tos-cli",
        "npm": "npm install -g tos-cli",
        "pip": "pip install tos-cli",
        "brew": "brew install tos-cli",
        "winget": "winget install tos-cli",
    },
    "ve-adrive-cli": {
        "command": "ve-adrive-cli",
        "cargo": "cargo install ve-adrive-cli",
        "npm": "npm install -g ve-adrive-cli",
        "pip": "pip install ve-adrive-cli",
        "brew": "brew install ve-adrive-cli",
        "winget": "winget install ve-adrive-cli",
    },
}


def test_public_cli_skills_are_installable_from_repo_paths():
    for skill_name, skill_info in PUBLIC_SKILLS.items():
        skill_dir = REPO_ROOT / "skills" / skill_name
        skill_md = skill_dir / "SKILL.md"
        command_name = skill_info["command"]

        assert skill_md.exists()
        content = skill_md.read_text(encoding="utf-8")
        assert content.startswith("---\n")
        assert f"name: {skill_name}\n" in content
        assert "description: " in content
        assert command_name in content
        assert "Volcengine Storage CLI" not in content
        assert "Volcengine Storage Unified CLI" not in content


def test_public_cli_skills_explain_binary_lookup_and_installation():
    for skill_name, skill_info in PUBLIC_SKILLS.items():
        content = (REPO_ROOT / "skills" / skill_name / "SKILL.md").read_text(encoding="utf-8")
        command_name = skill_info["command"]

        assert f"`{command_name} --version`" in content
        assert "Do not run storage operations if the binary is missing" in content
        assert "CLI installation" in content
        assert "brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli" in content
        assert skill_info["cargo"] in content
        assert skill_info["npm"] in content
        assert skill_info["pip"] in content
        assert skill_info["brew"] in content
        assert skill_info["winget"] in content
        assert f"sh -s -- {command_name}" in content


def test_public_cli_skills_are_self_contained_for_individual_install():
    for skill_name in PUBLIC_SKILLS:
        skill_dir = REPO_ROOT / "skills" / skill_name

        assert (skill_dir / "agents" / "openai.yaml").exists()
        assert (skill_dir / "references" / "safety.md").exists()


def test_legacy_generated_skill_catalog_is_not_checked_in_as_installable_skill():
    assert not (REPO_ROOT / "skill" / "SKILL.md").exists()
