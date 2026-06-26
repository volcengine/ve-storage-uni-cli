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

"""Registry/capabilities helpers for E2E coverage audits."""

from __future__ import annotations

from collections import defaultdict
from typing import Any, Iterable


UTILITY_ROOTS = {"api", "capabilities", "completion", "config", "doctor", "serve", "skill"}
TESTED_UTILITY_ROOTS = {"api", "config"}

HIGH_LEVEL_ROOTS = {
    "cat",
    "cp",
    "du",
    "find",
    "ls",
    "mb",
    "mkdir",
    "mv",
    "presign",
    "put",
    "rb",
    "restore",
    "rm",
    "stat",
    "sync",
}
BUCKET_BASIC_ROOTS = {"bucket", "quota", "storageclass", "redundancy-transition"}
BUCKET_CONFIG_ROOTS = {
    "access-monitor",
    "acl",
    "cdn-notification",
    "cors",
    "custom-domain",
    "encryption",
    "https-config",
    "intelligent-tiering",
    "inventory",
    "lifecycle",
    "logging",
    "max-age",
    "mirror",
    "notification",
    "pay-by-traffic",
    "payment",
    "policy",
    "real-time-log",
    "rename",
    "replication",
    "tagging",
    "transfer-acceleration",
    "trash",
    "versioning",
    "website",
    "worm",
}
CORE_OBJECT_ROOTS = {"multipart", "object", "turbo"}
CONTROL_ROOTS = {"accelerator", "ap", "cap", "control", "dataset", "mrap"}
DATA_PROCESS_ROOTS = {"data-process"}
OBJECT_SET_ROOTS = {"object-set"}


def flatten_commands(nodes: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
    flattened: list[dict[str, Any]] = []
    for node in nodes:
        flattened.append(node)
        flattened.extend(flatten_commands(node.get("subcommands") or []))
    return flattened


def leaf_commands(nodes: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
    return [node for node in flatten_commands(nodes) if not node.get("subcommands")]


def command_root(command: str) -> str:
    parts = command.split()
    if len(parts) < 2 or parts[0] not in {"tos", "ve-tos"}:
        raise ValueError(f"unexpected command shape: {command}")
    return parts[1]


def is_user_requested_e2e_root(root: str) -> bool:
    """Return whether the root belongs to the requested E2E scope.

    Utility commands are intentionally restricted to config/api per user request.
    """

    if root in UTILITY_ROOTS:
        return root in TESTED_UTILITY_ROOTS
    return True


def plan_group_for_root(root: str) -> str:
    if root in HIGH_LEVEL_ROOTS:
        return "HL"
    if root in CORE_OBJECT_ROOTS:
        return "OBJ"
    if root in BUCKET_BASIC_ROOTS:
        return "BB"
    if root in BUCKET_CONFIG_ROOTS:
        return "BC"
    if root in CONTROL_ROOTS:
        return "CTL"
    if root in DATA_PROCESS_ROOTS:
        return "DP"
    if root in OBJECT_SET_ROOTS:
        return "OS"
    if root in TESTED_UTILITY_ROOTS:
        return "CFG" if root == "config" else "AGT"
    return "UNKNOWN"


def commands_by_plan_group(leaves: Iterable[dict[str, Any]]) -> dict[str, list[dict[str, Any]]]:
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for leaf in leaves:
        root = command_root(leaf["command"])
        if is_user_requested_e2e_root(root):
            grouped[plan_group_for_root(root)].append(leaf)
    return dict(grouped)
