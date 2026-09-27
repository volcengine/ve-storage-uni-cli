from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
PUBLIC_SKILLS = {
    "ve-tos-cli": {
        "command": "ve-tos-cli",
        "cargo": "cargo install ve-tos-cli",
        "npm": "npm install -g ve-tos-cli",
        "pip": "pip install ve-tos-cli",
        "brew": "brew install ve-tos-cli",
    },
    "tos-cli": {
        "command": "tos-cli",
        "cargo": "cargo install tos-cli",
        "npm": "npm install -g tos-cli",
        "pip": "pip install tos-cli",
        "brew": "brew install tos-cli",
    },
    "ve-adrive-cli": {
        "command": "ve-adrive-cli",
        "cargo": "cargo install ve-adrive-cli",
        "npm": "npm install -g ve-adrive-cli",
        "pip": "pip install ve-adrive-cli",
        "brew": "brew install ve-adrive-cli",
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
        assert "Do not run storage operations if the binary is missing" in " ".join(content.split())
        assert "CLI installation" in content
        assert "references/installation.md" not in content
        assert "brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli" in content
        assert skill_info["cargo"] in content
        assert skill_info["npm"] in content
        assert skill_info["pip"] in content
        assert skill_info["brew"] in content
        assert "winget" not in content.lower()
        assert f"sh -s -- {command_name}" in content


def test_public_cli_skills_are_self_contained_for_individual_install():
    for skill_name in PUBLIC_SKILLS:
        skill_dir = REPO_ROOT / "skills" / skill_name

        assert (skill_dir / "agents" / "openai.yaml").exists()
        assert (skill_dir / "references" / "safety.md").exists()


def test_adrive_skill_documents_oauth_space_ownership_workflows():
    content = (REPO_ROOT / "skills" / "ve-adrive-cli" / "SKILL.md").read_text(
        encoding="utf-8"
    )

    # [Review Fix #Install1] Reflowing prose must not invalidate the OAuth contract check.
    content = " ".join(content.split())
    assert "description: Use when" in content
    assert "auth login" in content
    assert "--service-type" in content
    assert "--owner-type user" in content
    assert "--owner-type group" in content
    # [Review Fix #2] Retrieval must include an executable missing-identity
    # remedy and make the OAuth-bound Instance placeholder unambiguous.
    assert "does not provide `user_id`" in content
    assert (
        "run `ve-adrive-cli --auth-mode oauth auth login --instance instance-id` again"
        in content
    )
    assert "run `ve-adrive-cli --auth-mode oauth auth login` again" not in content
    assert "pass `--owner-id` explicitly" in content
    assert "OAuth-authorized Instance ID" in content
    assert "ve-adrive-cli --auth-mode oauth ls adrive://instance-id" in content
    assert (
        "ve-adrive-cli --auth-mode oauth ls adrive://instance-id --owner-type group"
        in content
    )


def test_legacy_generated_skill_catalog_is_not_checked_in_as_installable_skill():
    assert not (REPO_ROOT / "skill" / "SKILL.md").exists()


def test_public_skills_route_tasks_to_local_workflow_references():
    for skill_name in PUBLIC_SKILLS:
        skill_dir = REPO_ROOT / "skills" / skill_name
        content = (skill_dir / "SKILL.md").read_text(encoding="utf-8")
        assert "[Task workflows](references/workflows.md)" in content
        workflows = (skill_dir / "references/workflows.md").read_text(encoding="utf-8")
        for scenario in ("Upload and download", "Sync", "Delete", "Failure and verification"):
            assert scenario in workflows


def test_public_skills_explain_targeted_discovery_and_version_mismatch():
    for skill_name, skill_info in PUBLIC_SKILLS.items():
        content = (REPO_ROOT / "skills" / skill_name / "SKILL.md").read_text(encoding="utf-8")
        command_name = skill_info["command"]
        assert f"{command_name} cp --describe" in content
        assert f"{command_name} skill list" in content
        assert "installed version" in content
        assert "export" in content


def test_signed_url_safety_allows_authorized_delivery_without_logging():
    for skill_name in PUBLIC_SKILLS:
        safety = (REPO_ROOT / "skills" / skill_name / "references/safety.md").read_text(encoding="utf-8")
        assert "authorized user" in safety
        assert "logs or final answers" not in safety
        assert "logs" in safety


def test_adrive_sync_examples_do_not_use_unsupported_recursive_flag():
    skill_dir = REPO_ROOT / "skills" / "ve-adrive-cli"
    for document_path in skill_dir.rglob("*.md"):
        content = document_path.read_text(encoding="utf-8").replace("\\\n", " ")
        for line in content.splitlines():
            if "ve-adrive-cli sync " in line:
                assert "--recursive" not in line


def test_inline_installation_explains_channel_selection_and_verification():
    for skill_name in PUBLIC_SKILLS:
        directory = REPO_ROOT / "skills" / skill_name
        entry = (directory / "SKILL.md").read_text(encoding="utf-8")
        assert not (directory / "references/installation.md").exists()
        guide = entry.split("## CLI installation", 1)[1].split("## Task routing", 1)[0]
        assert "does not install the CLI" in entry
        assert "Choose one" in guide
        for channel in ("Homebrew", "npm", "pip", "Cargo", "install script"):
            assert channel in guide
        assert f"{skill_name} --version" in guide
        assert "PATH" in guide
        assert "winget" not in entry.lower()


def test_bytetos_internal_downloads_are_versioned_and_scoped():
    relative_urls = ("linux/tos-cli", "mac/tos-cli", "win/tos-cli.exe")
    for skill_name in PUBLIC_SKILLS:
        guide = (REPO_ROOT / "skills" / skill_name / "SKILL.md").read_text(encoding="utf-8")
        if skill_name == "tos-cli":
            assert "ByteCloud internal network" in guide
            assert "1.0.2" in guide
            for suffix in relative_urls:
                assert f"https://tosv.byted.org/obj/tos-team/toscli/new/1.0.2/{suffix}" in guide
            assert "chmod +x" in guide
            assert ".\\tos-cli.exe --version" in guide
        else:
            assert "tosv.byted.org" not in guide


def test_tos_skill_documents_public_zti_without_changing_ve_skills():
    tos_dir = REPO_ROOT / "skills" / "tos-cli"
    guide = (tos_dir / "SKILL.md").read_text(encoding="utf-8")
    workflows = (tos_dir / "references" / "workflows.md").read_text(encoding="utf-8")
    for expected in (
        "--auth-mode zti",
        "SEC_TOKEN_STRING",
        "SEC_TOKEN_PATH",
        "ZTI_AGENT_SOCKET_PATH",
        "ZTI does not support presign",
    ):
        assert expected in guide
    assert "ZTI does not support presign" in workflows
    for skill_name in ("ve-tos-cli", "ve-adrive-cli"):
        other = (REPO_ROOT / "skills" / skill_name / "SKILL.md").read_text(encoding="utf-8")
        assert "SEC_TOKEN_STRING" not in other
        assert "ZTI_AGENT_SOCKET_PATH" not in other
