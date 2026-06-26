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

# [G11] Skill catalog generator.
#
# Materialises `skill/SKILL.md` from the live `ve-tos-cli skill list` registry so
# the file an Agent reads always stays in lock-step with the binary it is
# driving.
#
# Usage:
#     python3 scripts/generate_skill_md.py
#     python3 scripts/generate_skill_md.py --check     # CI mode: exit 1 if drift
#     python3 scripts/generate_skill_md.py --skills-json path/to/cached.json
#
# The script either invokes the `packaging/cargo/ve-tos-cli` dedicated entry
# crate itself, or reads pre-baked JSON from --skills-json (used in tests so the
# generator is deterministic without invoking the build).

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = REPO_ROOT / "skill" / "SKILL.md"


def fetch_skills_via_cargo() -> Any:
    """Run the CLI to obtain the live skill catalog."""
    # Use the dedicated TOS entry crate so the generated skill catalog follows
    # the same direct invocation surface users get from `cargo install ve-tos-cli`.
    cmd = [
        "cargo",
        "run",
        "-q",
        "--manifest-path",
        "packaging/cargo/ve-tos-cli/Cargo.toml",
        "--",
        "skill",
        "list",
        "--output",
        "json",
    ]
    proc = subprocess.run(
        cmd, cwd=REPO_ROOT, check=True, capture_output=True, text=True
    )
    return json.loads(proc.stdout)


def load_skills(args: argparse.Namespace) -> Any:
    if args.skills_json:
        path = Path(args.skills_json)
        return json.loads(path.read_text(encoding="utf-8"))
    return fetch_skills_via_cargo()


def envelope_data(payload: Any) -> Any:
    if isinstance(payload, dict) and "status" in payload and "data" in payload:
        return payload["data"]
    return payload


RISK_BADGE = {
    "low": "🟢 low",
    "medium": "🟡 medium",
    "high": "🟠 high",
    "critical": "🔴 critical",
    "unknown": "⚪ unknown",
}


def render_param_table(input_schema: dict[str, Any]) -> str:
    if not input_schema:
        return "_(no input schema)_\n"
    properties = input_schema.get("properties") or {}
    required = set(input_schema.get("required") or [])
    if not properties:
        return "_(no parameters)_\n"
    lines = ["| Name | Type | Required | Description |", "| --- | --- | --- | --- |"]
    for name, meta in properties.items():
        meta = meta or {}
        ty = meta.get("type") or "string"
        desc = (meta.get("description") or "").replace("|", "\\|")
        req = "yes" if name in required else "no"
        lines.append(f"| `{name}` | `{ty}` | {req} | {desc} |")
    return "\n".join(lines) + "\n"


def render_skill(skill: dict[str, Any]) -> str:
    name = skill.get("name") or "(unnamed)"
    command = skill.get("command") or ""
    description = (skill.get("description") or "").strip()
    risk = (skill.get("risk_level") or "unknown").lower()
    badge = RISK_BADGE.get(risk, RISK_BADGE["unknown"])
    examples = skill.get("examples") or []
    out = [f"### `{name}`", ""]
    if command:
        out.append(f"**Command:** `{command}`")
    out.append(f"**Risk:** {badge}")
    out.append("")
    if description:
        out.append(description)
        out.append("")
    out.append("**Parameters:**")
    out.append("")
    out.append(render_param_table(skill.get("input_schema") or {}))
    if examples:
        out.append("**Examples:**")
        out.append("")
        for example in examples:
            out.append(f"```bash\n{example}\n```")
        out.append("")
    return "\n".join(out)


def render_markdown(skills_payload: Any) -> str:
    data = envelope_data(skills_payload)
    skills = data.get("skills") if isinstance(data, dict) else None
    if not isinstance(skills, list):
        raise SystemExit("Unexpected skill catalog shape: missing `skills` array")

    # Group by binary ("tos object list" -> "tos object").
    groups: dict[str, list[dict[str, Any]]] = {}
    for skill in skills:
        cmd = skill.get("command") or ""
        parts = cmd.split()
        group = " ".join(parts[:2]) if len(parts) >= 2 else (parts[0] if parts else "(misc)")
        groups.setdefault(group, []).append(skill)

    lines = [
        "<!--",
        "Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.",
        "",
        'Licensed under the Apache License, Version 2.0 (the "License");',
        "you may not use this file except in compliance with the License.",
        "You may obtain a copy of the License at",
        "",
        "http://www.apache.org/licenses/LICENSE-2.0",
        "",
        "Unless required by applicable law or agreed to in writing, software",
        'distributed under the License is distributed on an "AS IS" BASIS,',
        "WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.",
        "See the License for the specific language governing permissions and",
        "limitations under the License.",
        "-->",
        "",
        "# TOS Unified CLI - MCP Skill Definitions",
        "",
        "> Auto-generated by `scripts/generate_skill_md.py` from `ve-tos-cli skill list`.",
        "> Do not edit manually — re-run the generator after changing the registry.",
        "",
        "## Overview",
        "",
        "The TOS Unified CLI exposes its commands as MCP (Model Context Protocol)",
        "tools so AI agents can drive Volcengine TOS object storage with the same",
        "shape they see in `ve-tos-cli --help`.",
        "",
        f"This catalog ships **{len(skills)} skills** across **{len(groups)} command",
        "groups**.",
        "",
        "## Tool Schema Format",
        "",
        "Each skill is rendered as an MCP tool definition:",
        "",
        "```json",
        "{",
        "  \"name\": \"tos_bucket_create\",",
        "  \"description\": \"Create a new TOS bucket\",",
        "  \"inputSchema\": { \"type\": \"object\", \"properties\": { ... }, \"required\": [ ... ] }",
        "}",
        "```",
        "",
        "Risk levels follow the safe-execution gate:",
        "",
        "- 🟢 **low** — read-only / observational",
        "- 🟡 **medium** — mutates configuration",
        "- 🟠 **high** — mutates data",
        "- 🔴 **critical** — irreversible destructive (requires `--confirm`)",
        "",
        "## Skills",
        "",
    ]

    for group in sorted(groups):
        items = groups[group]
        lines.append(f"### Group: `{group}` ({len(items)} skills)")
        lines.append("")
        for skill in sorted(items, key=lambda s: s.get("command") or ""):
            lines.append(render_skill(skill))

    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate skill/SKILL.md")
    parser.add_argument("--output", default=str(DEFAULT_OUTPUT))
    parser.add_argument(
        "--skills-json",
        help="Read the catalog from this file instead of running cargo",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Exit 1 if the regenerated file would differ from --output",
    )
    args = parser.parse_args()

    payload = load_skills(args)
    rendered = render_markdown(payload)
    output = Path(args.output)

    if args.check:
        existing = output.read_text(encoding="utf-8") if output.exists() else ""
        if existing != rendered:
            sys.stderr.write(
                f"::error::{output} is out of date. Re-run "
                f"`python3 scripts/generate_skill_md.py`.\n"
            )
            return 1
        return 0

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered, encoding="utf-8")
    print(f"Wrote {output} ({len(rendered.splitlines())} lines)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
