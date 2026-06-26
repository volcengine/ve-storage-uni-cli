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

"""Metadata-driven surface coverage helpers for E2E dry-run tests."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping

from .registry import (
    CONTROL_ROOTS,
    DATA_PROCESS_ROOTS,
    OBJECT_SET_ROOTS,
    command_root,
    is_user_requested_e2e_root,
    leaf_commands,
    plan_group_for_root,
)


CommandNode = Mapping[str, Any]
ParameterNode = Mapping[str, Any]

RFC_1123_TIME = "Wed, 21 Oct 2030 07:28:00 GMT"
ISO_TIME = "2030-01-01T00:00:00Z"


@dataclass(frozen=True)
class SurfaceCase:
    """A generated dry-run case tied to one leaf command surface."""

    case_id: str
    command: str
    group: str
    args: tuple[str, ...]
    covered_parameters: frozenset[str]


def in_scope_leaf_commands(command_tree: Iterable[CommandNode]) -> list[dict[str, Any]]:
    """Return unique leaf commands that belong to the requested E2E scope."""

    seen: set[str] = set()
    leaves: list[dict[str, Any]] = []
    raw_nodes = [dict(node) for node in command_tree]
    for leaf in leaf_commands(raw_nodes):
        command = leaf["command"]
        if command in seen:
            continue
        seen.add(command)
        if is_user_requested_e2e_root(command_root(command)):
            leaves.append(leaf)
    return leaves


def manual_utility_parameter_coverage() -> dict[str, set[str]]:
    """Utility coverage still comes from explicit hand-written tests."""

    return {
        "ve-tos config init": {"profile"},
        "ve-tos config set": {"key", "value"},
        "ve-tos config show": set(),
        "ve-tos api": {"group", "action", "request", "describe", "force"},
    }


def build_surface_cases(leaf: CommandNode, tmp_path: Path) -> list[SurfaceCase]:
    """Build one or more dry-run invocations that cover the leaf command surface."""

    command = str(leaf["command"])
    group = plan_group_for_root(command_root(command))
    params = [dict(param) for param in leaf.get("parameters") or []]
    positional = [param for param in params if param.get("positional")]
    named = [param for param in params if param.get("long")]

    identity_variants = _expand_parameter_modes(
        command,
        _identity_variants(command, positional, named, tmp_path),
    )
    cases: list[SurfaceCase] = []
    base_slug = _slug(command)
    for index, variant in enumerate(identity_variants, start=1):
        args = command.split()
        args.extend(variant.args)
        covered = set(variant.covered_parameters)
        excluded = set(variant.excluded_parameters)
        for param in named:
            key = _parameter_key(param)
            if key in excluded or key in covered:
                continue
            args.append(f"--{param['long']}")
            covered.add(key)
            if param.get("takes_value", True):
                args.append(_parameter_value(command, param, tmp_path))
        cases.append(
            SurfaceCase(
                case_id=f"{group}-AUTO-{base_slug}-{index}",
                command=command,
                group=group,
                args=tuple(args),
                covered_parameters=frozenset(covered),
            )
        )
    return cases


def _expand_parameter_modes(command: str, variants: list["_IdentityVariant"]) -> list["_IdentityVariant"]:
    mode_exclusions = _mode_exclusions(command)
    if not mode_exclusions:
        return variants

    expanded: list[_IdentityVariant] = []
    for variant in variants:
        for excluded in mode_exclusions:
            expanded.append(
                _IdentityVariant(
                    args=variant.args,
                    covered_parameters=variant.covered_parameters,
                    excluded_parameters=frozenset(set(variant.excluded_parameters) | excluded),
                )
            )
    return expanded


def _mode_exclusions(command: str) -> list[set[str]]:
    logical_command = _logical_command(command)
    if logical_command == "ve-tos bucket delete":
        return [{"destroy"}, {"force"}]
    if logical_command == "ve-tos acl set":
        grant_header_parameters = {
            "grant-full-control",
            "grant-read",
            "grant-read-non-list",
            "grant-read-acp",
            "grant-write",
            "grant-write-acp",
        }
        all_header_parameters = {"acl"} | grant_header_parameters
        return [
            all_header_parameters,
            {"config"} | grant_header_parameters,
            {"config", "acl"},
        ]
    return []


def build_smoke_case(leaf: CommandNode, tmp_path: Path) -> SurfaceCase:
    """Build the minimal dry-run invocation for leaf-command execution coverage."""

    command = str(leaf["command"])
    group = plan_group_for_root(command_root(command))
    root = command_root(command)
    if root in CONTROL_ROOTS | DATA_PROCESS_ROOTS | OBJECT_SET_ROOTS:
        args = tuple(command.split()) + _advanced_generic_smoke_args()
        return SurfaceCase(
            case_id=f"{group}-SMOKE-{_slug(command)}",
            command=command,
            group=group,
            args=args,
            covered_parameters=frozenset(),
        )

    params = [dict(param) for param in leaf.get("parameters") or []]
    positional = [param for param in params if param.get("positional")]
    named = [param for param in params if param.get("long")]
    variant = _identity_variants(command, positional, named, tmp_path)[0]
    return SurfaceCase(
        case_id=f"{group}-SMOKE-{_slug(command)}",
        command=command,
        group=group,
        args=tuple(command.split()) + variant.args,
        covered_parameters=variant.covered_parameters,
    )


def build_execution_case(leaf: CommandNode, tmp_path: Path) -> SurfaceCase:
    """Build the execution-oriented case used by the full leaf-command smoke suite."""

    command = str(leaf["command"])
    root = command_root(command)
    if root in CONTROL_ROOTS | DATA_PROCESS_ROOTS | OBJECT_SET_ROOTS:
        return build_smoke_case(leaf, tmp_path)
    return build_surface_cases(leaf, tmp_path)[0]


def parameter_keys(leaf: CommandNode) -> set[str]:
    return {_parameter_key(param) for param in leaf.get("parameters") or []}


def action_matches_command(action: str, command: str) -> bool:
    """Return whether an internal dry-run action matches a public capability command."""

    candidates = {
        command,
        command.removeprefix("ve-tos "),
        _logical_command(command),
        _logical_command(command).removeprefix("ve-tos "),
    }
    return any(candidate and action.startswith(candidate) for candidate in candidates)


def expected_validation_failure(expected_failures: Mapping[str, str], command: str) -> str | None:
    return expected_failures.get(command) or expected_failures.get(_logical_command(command))


def _logical_command(command: str) -> str:
    return command


def _identity_variants(
    command: str,
    positional: list[dict[str, Any]],
    named: list[dict[str, Any]],
    tmp_path: Path,
) -> list["_IdentityVariant"]:
    named_keys = {_parameter_key(param) for param in named}
    positional_keys = [_parameter_key(param) for param in positional]

    if "uri" in positional_keys and "bucket" in named_keys:
        uri_variant = _IdentityVariant(
            args=(_positional_value(command, positional[0], 0, tmp_path),),
            covered_parameters=frozenset({"uri"}),
            excluded_parameters=frozenset({"bucket", "key", "prefix"}),
        )
        named_variant_args = ["--bucket", _bucket_value(command)]
        covered = {"bucket"}
        excluded = {"uri"}
        if "key" in named_keys:
            named_variant_args.extend(["--key", _object_key_value(command)])
            covered.add("key")
        if "prefix" in named_keys:
            named_variant_args.extend(["--prefix", "prefix/"])
            covered.add("prefix")
        return [
            uri_variant,
            _IdentityVariant(
                args=tuple(named_variant_args),
                covered_parameters=frozenset(covered),
                excluded_parameters=frozenset(excluded),
            ),
        ]

    if "path" in positional_keys and "bucket" in named_keys:
        path_param = positional[positional_keys.index("path")]
        path_variant = _IdentityVariant(
            args=(_positional_value(command, path_param, 0, tmp_path),),
            covered_parameters=frozenset({"path"}),
            excluded_parameters=frozenset({"bucket", "key", "prefix"}),
        )
        named_variant_args = ["--bucket", _bucket_value(command)]
        covered = {"bucket"}
        excluded = {"path"}
        if "key" in named_keys:
            named_variant_args.extend(["--key", _object_key_value(command)])
            covered.add("key")
        if "prefix" in named_keys:
            named_variant_args.extend(["--prefix", "prefix/"])
            covered.add("prefix")
        return [
            path_variant,
            _IdentityVariant(
                args=tuple(named_variant_args),
                covered_parameters=frozenset(covered),
                excluded_parameters=frozenset(excluded),
            ),
        ]

    # [Review Fix] bucket_pos (positional) and bucket (--bucket flag) are aliases;
    # generate two mutually exclusive variants like uri/bucket above.
    if any(k.startswith("bucket") for k in positional_keys) and "bucket" in named_keys:
        pos_param = next(p for p in positional if str(p.get("name", "")).lower().startswith("bucket"))
        pos_key = _parameter_key(pos_param)
        pos_variant = _IdentityVariant(
            # [Review Fix #TOS-E2E-BucketURI] Bucket positional parameters must
            # exercise the strict URI form; bare bucket names belong to --bucket.
            args=(_bucket_uri_value(command),),
            covered_parameters=frozenset({pos_key}),
            excluded_parameters=frozenset({"bucket"}),
        )
        flag_variant = _IdentityVariant(
            args=("--bucket", _bucket_value(command)),
            covered_parameters=frozenset({"bucket"}),
            excluded_parameters=frozenset({pos_key}),
        )
        return [pos_variant, flag_variant]

    args: list[str] = []
    covered: set[str] = set()
    for index, param in enumerate(positional):
        args.append(_positional_value(command, param, index, tmp_path))
        covered.add(_parameter_key(param))

    if not positional:
        for param in named:
            key = _parameter_key(param)
            if not param.get("required"):
                continue
            args.append(f"--{param['long']}")
            covered.add(key)
            if param.get("takes_value", True):
                args.append(_parameter_value(command, param, tmp_path))

    return [_IdentityVariant(args=tuple(args), covered_parameters=frozenset(covered), excluded_parameters=frozenset())]


@dataclass(frozen=True)
class _IdentityVariant:
    args: tuple[str, ...]
    covered_parameters: frozenset[str]
    excluded_parameters: frozenset[str]


def _parameter_key(param: ParameterNode) -> str:
    long = param.get("long")
    if isinstance(long, str) and long:
        return long
    name = param.get("name")
    if isinstance(name, str) and name:
        return name
    raise AssertionError(f"parameter without key: {param!r}")


def _slug(command: str) -> str:
    return "-".join(command.split()[1:])


def _positional_value(command: str, param: ParameterNode, index: int, tmp_path: Path) -> str:
    name = str(param.get("name") or "").lower()
    if command_root(command) in {"mb", "rb"} and name.startswith("bucket"):
        # [Review Fix #3] `ve-tos mb/rb` generated execution must use the strict
        # URI form; a bare bucket makes the full-surface test fall back to
        # describe instead of exercising dry-run execution.
        return _bucket_uri_value(command)
    if name == "uri":
        return _uri_value(command)
    if name == "source":
        return _source_value(command, tmp_path)
    if name == "destination":
        return _destination_value(command, tmp_path)
    if name == "path":
        return _uri_value(command)
    if name == "key":
        return "default.region"
    if name == "value":
        return "cn-beijing"
    if name == "group":
        return "bucket"
    if name == "action":
        return "create"
    return _generic_value(command, name, tmp_path, positional_index=index)


def _parameter_value(command: str, param: ParameterNode, tmp_path: Path) -> str:
    key = _parameter_key(param).lower()
    return _generic_value(command, key, tmp_path)


def _generic_value(command: str, key: str, tmp_path: Path, positional_index: int | None = None) -> str:
    if key in {"bucket", "bucket-name", "target-bucket", "destination-bucket"}:
        return _bucket_value(command)
    if key in {"key", "object", "target-key", "source-key"}:
        return _object_key_value(command)
    if key in {"body"}:
        return _body_value(command, tmp_path)
    if key in {"request"}:
        return '{"bucket":"dry-run-bucket"}'
    if key in {"config"}:
        return _config_value(command)
    if key in {"profile"}:
        return "default"
    if key in {"region", "destination-location"}:
        return "cn-beijing"
    if key in {"endpoint"}:
        return "https://tos-cn-beijing.volces.com"
    if key in {"control-endpoint"}:
        return "https://tos-control-cn-beijing.volces.com"
    if key in {"storage-class", "destination-storage-class"}:
        return "STANDARD"
    if key == "recursive-delete-mode":
        return "bottom-up"
    logical_command = _logical_command(command)
    if key == "mode" and logical_command == "ve-tos turbo open":
        return "0"
    if key == "mode":
        return "COMPLIANCE"
    if key == "storage-class-inherit-directive":
        return "DESTINATION_BUCKET"
    if key == "bucket-type":
        return "fns"
    if key == "acl":
        return "private"
    if key.startswith("grant-"):
        return "id=e2e"
    if key in {"meta", "tagging", "tags", "if-match-tags", "persistent-headers", "header"}:
        return "k=v"
    if key == "query":
        return "foo==bar"
    if key in {"keys"}:
        return '["a","b"]'
    if key in {"parts"}:
        return '[{"PartNumber":1,"ETag":"etag"}]'
    if key in {"copy-source", "source-url"}:
        return "/dry-run-bucket/source-object"
    if key in {"copy-source-range", "range"}:
        return "bytes=0-3" if key == "copy-source-range" else "0-3"
    if key in {"if-modified-since", "if-unmodified-since", "copy-source-if-modified-since", "copy-source-if-unmodified-since"}:
        return RFC_1123_TIME
    if key in {"object-lock-retain-until-date"}:
        return ISO_TIME
    if key in {"expires", "days", "part-number", "part-number-marker", "max-parts", "max-uploads", "max-keys", "max-depth", "offset", "traffic-limit", "decoded-content-length", "append-last-time", "time", "modify-timestamp", "modify-timestamp-ns", "crr-source-last-modify-time", "crr-source-timestamp-nsec", "copy-source-last-modified", "copy-source-part-number", "if-match-expires", "last-modified", "if-match-create-time", "if-match-access-time", "inner-properties-timestamp", "inner-properties-timestamp-nsec"}:
        return "1"
    if key in {"hash-crc64ecma"}:
        return "1"
    if key in {"content-md5"}:
        return "deadbeef"
    if key in {"content-sha256"}:
        return "abc"
    if key in {"content-type"}:
        return "text/plain"
    if key in {"name", "alias", "project-name", "style-name"}:
        return "e2e-name"
    if key in {"id", "job-id", "task-id", "upload-id", "version-id", "accelerator-id"}:
        return "e2e-id"
    if key in {"job-type"}:
        return "image"
    if key in {"accelerator"}:
        return "e2e-accelerator"
    if key in {"resource-trn"}:
        return "trn:e2e"
    if key in {"tag-keys"}:
        return "k1,k2"
    if key in {"tag"}:
        return "Transcode"
    if key in {"domain"}:
        return "example.com"
    if key in {"az"}:
        return "cn-beijing-a"
    if key in {"az-redundancy"}:
        return "single-az"
    if key in {"encoding-type"}:
        return "url"
    if key in {"prefix"}:
        return "prefix/"
    if key in {"delimiter"}:
        return "/"
    if key in {"continuation-token", "marker", "upload-id-marker"}:
        return "token"
    if key in {"etag-pattern", "if-match", "if-none-match", "if-match-guard-object"}:
        return "etag"
    if key in {"object-lock-mode"}:
        return "GOVERNANCE"
    if key in {"crr-source-bucket-version-status"}:
        return "Enabled"
    if key in {"replicated-from", "crr-proxy", "crr-source-version-id", "trace-id", "turbo-token", "finger-print", "data-id", "unique-tag", "if-match-inode-id", "parent-inode-id"}:
        return "e2e-token"
    if key in {"forbid-overwrite", "recursive", "checkpoint", "no-progress", "force", "bucket-object-lock-enabled", "destroy", "human-readable", "fetch-from-kv", "from-modular", "lifecycle-directly-delete-versions", "only-put-delete-marker", "recursive-mkdir", "not-update-timestamp", "skip-trash", "use-service-topic", "parents"}:
        return "true"
    if key in {"checkpoint-dir"}:
        path = tmp_path / "checkpoints"
        path.mkdir(parents=True, exist_ok=True)
        return str(path)
    if key in {"report-path"}:
        return str(tmp_path / "report.jsonl")
    if key in {"path"}:
        return "/"
    if key in {"net-speed-test"}:
        return "true"
    if key in {"bandwidth-limit"}:
        return "1MB"
    if key in {"mtime"}:
        return "1"
    if key in {"size"}:
        return "+1KB"
    if key in {"tier"}:
        return "Standard"
    if key in {"method"}:
        return "PUT"
    if key in {"object-set-name"}:
        return "object-set"
    if key in {"source", "destination"}:
        if positional_index == 0:
            return _source_value(command, tmp_path)
        return _destination_value(command, tmp_path)
    return "e2e"


def _source_value(command: str, tmp_path: Path) -> str:
    root = command_root(command)
    if root == "sync":
        source_dir = tmp_path / "sync-src"
        source_dir.mkdir(parents=True, exist_ok=True)
        (source_dir / "source.txt").write_text("e2e-source\n")
        return str(source_dir)
    if root in {"cp", "mv"}:
        source = tmp_path / "source.txt"
        source.write_text("e2e-source\n")
        return str(source)
    return _uri_value(command)


def _destination_value(command: str, tmp_path: Path) -> str:
    root = command_root(command)
    if root == "sync":
        return "tos://dry-run-bucket/prefix/"
    if root in {"cp", "mv"}:
        return "tos://dry-run-bucket/destination"
    output = tmp_path / "output.bin"
    return str(output)


def _uri_value(command: str) -> str:
    root = command_root(command)
    logical_command = _logical_command(command)
    if logical_command == "ve-tos object list":
        return "tos://dry-run-bucket/prefix/"
    if _is_bucket_only_uri_command(command):
        return _bucket_uri_value(command)
    if root in {
        "bucket",
        "quota",
        "policy",
        "lifecycle",
        "cors",
        "versioning",
        "storageclass",
        "encryption",
        "tagging",
        "acl",
        "rename",
        "transfer-acceleration",
        "trash",
        "payment",
        "logging",
        "max-age",
        "replication",
        "notification",
        "website",
        "mirror",
        "inventory",
        "custom-domain",
        "access-monitor",
        "worm",
        "real-time-log",
        "cdn-notification",
        "https-config",
        "intelligent-tiering",
        "pay-by-traffic",
        "redundancy-transition",
    }:
        return _bucket_uri_value(command)
    if root in {"mb", "rb"}:
        return _bucket_uri_value(command)
    return "tos://dry-run-bucket/e2e-object"


def _is_bucket_only_uri_command(command: str) -> bool:
    return _logical_command(command) in {
        "ve-tos object batch-delete",
        "ve-tos object list-versions",
        "ve-tos object get-fetch-task",
        "ve-tos object create-fetch-task",
        "ve-tos object fetch",
        "ve-tos multipart list",
        "ve-tos turbo list",
    }


def _bucket_uri_value(_command: str) -> str:
    return "tos://dry-run-bucket"


def _bucket_value(_command: str) -> str:
    return "dry-run-bucket"


def _object_key_value(command: str) -> str:
    root = command_root(command)
    if root == "turbo":
        return "turbo-object"
    return "e2e/object.txt"


def _body_value(command: str, tmp_path: Path) -> str:
    root = command_root(command)
    if root in {"object", "multipart", "turbo"} and any(
        token in command for token in ("download", "append", "upload", "open", "form-upload")
    ):
        payload = tmp_path / "payload.bin"
        payload.write_bytes(b"e2e-payload\n")
        return str(payload)
    payload = tmp_path / "body.bin"
    payload.write_bytes(b"e2e-body\n")
    return str(payload)


def _config_value(command: str) -> str:
    root = command_root(command)
    logical_command = _logical_command(command)
    if logical_command == "ve-tos data-process set-image-style-separator":
        return '{"Separator":["-"]}'
    if logical_command == "ve-tos data-process set-template":
        return '{"Name":"e2e-template","Tag":"Transcode","TranscodeConfig":{}}'
    if logical_command == "ve-tos dataset create":
        return '{"DatasetName":"e2e-dataset","TemplateId":"e2e-template"}'
    if logical_command == "ve-tos ap create":
        return '{"Bucket":"dry-run-bucket","NetworkOrigin":"internet"}'
    if root == "cdn-notification":
        return '{"Role":"role","Rules":[{"RuleId":"rule-1","CustomDomain":"example.com","Events":["tos:ObjectCreated:*"],"Filter":{"TOSKey":{"FilterRules":[{"Name":"prefix","Value":"e2e/"}]}}}]}'
    if root == "quota":
        return '{"Quota":1048576}'
    if root == "policy":
        return '{"Statement":[]}'
    if root == "versioning":
        return '{"Status":"Suspended"}'
    if root == "tagging":
        return '{"TagSet":[{"Key":"purpose","Value":"e2e"}]}'
    if root == "acl":
        return '{"Owner":{},"Grants":[]}'
    if root == "storageclass":
        return '{"StorageClass":"STANDARD"}'
    if root == "rename":
        return '{"Enabled":true}'
    if root == "transfer-acceleration":
        return '{"Status":"Suspended"}'
    if root == "trash":
        return '{"Status":"Disabled"}'
    if root == "payment":
        return '{"Payer":"BucketOwner"}'
    if root == "max-age":
        return '{"MaxAge":60}'
    return '{"name":"e2e"}'


def _advanced_generic_smoke_args() -> tuple[str, ...]:
    return (
        "--name",
        "e2e-name",
        "--bucket",
        "dry-run-bucket",
        "--id",
        "e2e-id",
        "--style-name",
        "style",
        "--job-id",
        "job",
        "--job-type",
        "image",
        "--alias",
        "alias",
        "--accelerator",
        "acc",
        "--accelerator-id",
        "acc-id",
        "--bucket-name",
        "dry-run-bucket",
        "--domain",
        "example.com",
        "--az",
        "cn-beijing-a",
        "--region",
        "cn-beijing",
        "--resource-trn",
        "trn:e2e",
        "--tag-keys",
        "k1,k2",
        "--tag",
        "Transcode",
        "--object-set-name",
        "object-set",
        "--object",
        "object-key",
        "--config",
        '{"name":"e2e","Name":"e2e","Bucket":"dry-run-bucket","NetworkOrigin":"internet","Separator":["-"],"Tag":"Transcode","TemplateId":"e2e-template","DatasetName":"e2e-dataset","TranscodeConfig":{}}',
        "--content-md5",
        "deadbeef",
        "--header",
        "x-test=v",
        "--force",
    )
