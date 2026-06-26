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

"""E2E case metadata helpers.

The Lark test-plan requires every executable flow to carry a stable ``@case-id``.
These helpers keep the IDs visible in pytest node IDs and easy to grep in CI.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Sequence


@dataclass(frozen=True)
class CommandCase:
    """A CLI command case with a traceable test-plan ID."""

    case_id: str
    group: str
    args: tuple[str, ...]
    expected_exit: int = 0
    dry_run: bool = False

    @classmethod
    def build(
        cls,
        case_id: str,
        group: str,
        args: Sequence[str],
        *,
        expected_exit: int = 0,
        dry_run: bool = False,
    ) -> "CommandCase":
        return cls(case_id, group, tuple(args), expected_exit, dry_run)
