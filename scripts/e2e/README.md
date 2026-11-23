<!--
Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# ve-storage-uni-cli E2E Test Suite

This directory contains Python-based end-to-end tests for the `ve-storage-uni-cli` binary
against real TOS endpoints. The suite validates the public CLI contract from
`argv -> exit code -> stdout/stderr -> envelope payload`, instead of bypassing the
CLI with a storage SDK.

## Scope

| Principle                | Test Files                                                                                                                                                                                               | Key Assertion                                                                                                                                                                                                                                |
|--------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Discovery                | `tos/test_discovery.py`                                                                                                                                                                                  | `--capabilities` and `--describe` remain available in a real environment                                                                                                                                                                     |
| Safe Execution           | `tos/test_bucket_lifecycle.py`                                                                                                                                                                           | destructive bucket deletion must require `--force` plus path-matched `--confirm` in non-interactive execution                                                                                                                               |
| Controlled Output        | `tos/test_envelope_schema.py`                                                                                                                                                                            | success responses expose `status/command/data`; `--query` projections stay deterministic                                                                                                                                                     |
| Deterministic Errors     | `tos/test_error_codes.py`                                                                                                                                                                                | 401 -> 2 / 404 -> 4 / validation -> 6                                                                                                                                                                                                        |
| Agent Ecosystem          | `tos/test_object_lifecycle.py`                                                                                                                                                                           | upload -> head -> download round-trip preserves object bytes and MD5                                                                                                                                                                         |
| Handwritten Matrix       | `tos/test_high_level_parameter_matrix.py` / `tos/test_core_object_matrix.py` / `tos/test_multipart_turbo_matrix.py` / `tos/test_bucket_config_matrix.py` / `tos/test_advanced_group_matrix.py`           | curated group-level cases exercise representative full-parameter flows across `HL/OBJ/BB/BC/CTL/DP/OS`                                                                                                                                       |
| Generated Surface Matrix | `tos/test_generated_surface_matrix.py`                                                                                                                                                                   | every in-scope non-utility leaf command is executed through a generated command vector; the runner prefers `--dry-run`, then falls back to `--describe`, then `--help` when a command is reachable but blocked by deeper business validation |
| Utilities                | `tos/test_utilities_config_api.py` / `tos/test_utilities_agent_audit.py` / `adrive/test_utilities_agent_audit.py`                                                                                        | Config/API flows plus registry-backed `completion`, `serve`, `skill`, `doctor`, and capabilities surfaces remain reachable and structured                                                                                                    |
| Global Args              | `tos/test_global_parameter_matrix.py`                                                                                                                                                                    | endpoint/profile/control-endpoint/query/output/quiet/trace global arguments parse correctly                                                                                                                                                  |
| Surface Guards           | `tos/test_parameter_surface.py` / `tos/test_teardown_contract.py`                                                                                                                                        | registry-backed parameter metadata, generated coverage, and destructive cleanup contracts remain complete                                                                                                                                    |

## Coverage Model

The suite uses two complementary layers:

- Handwritten scenario tests validate real workflows such as bucket lifecycle, object round-trip, config flow, and
  grouped command behavior.
- Metadata-driven tests derive command coverage from `ve-tos capabilities --view tree`, which acts as the single source of
  truth for leaf commands and parameter metadata.

The metadata-driven layer is split by responsibility:

- `test_generated_surface_matrix.py` proves that every in-scope non-utility leaf command is reachable from the CLI
  surface.
- `test_parameter_surface.py` proves that every declared in-scope parameter is passed by at least one E2E case.

This separation is intentional. Some commands accept a parameter in metadata but still
require business-valid combinations that are not suitable for a generic generated
`--dry-run` invocation. In those cases, execution coverage falls back to `--describe`
or `--help`, while parameter coverage is still enforced independently.

## Group Matrix

| Group | Command Roots                                                                                                                                                                                                                                                                                                                                                                               | Primary Tests                                                                                                                                             | Coverage Strategy                                                                                                                                                                                           |
|-------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `HL`  | `cat` / `cp` / `du` / `find` / `ls` / `mb` / `mv` / `presign` / `rb` / `restore` / `rm` / `stat` / `sync`                                                                                                                                                                                                                                                                                   | `tos/test_high_level_parameter_matrix.py` / `tos/test_generated_surface_matrix.py`                                                                        | Handwritten high-level full-parameter dry-run cases cover representative flows; generated coverage ensures every high-level leaf command remains reachable from the CLI surface                             |
| `BB`  | `bucket` / `quota` / `storageclass` / `redundancy-transition`                                                                                                                                                                                                                                                                                                                                | `tos/test_bucket_lifecycle.py` / `tos/test_bucket_config_matrix.py` / `tos/test_generated_surface_matrix.py`                                              | Real lifecycle tests validate create/head/list/delete behavior and teardown; generated coverage keeps all basic bucket leaf commands reachable                                                              |
| `BC`  | `access-monitor` / `acl` / `cdn-notification` / `cors` / `custom-domain` / `encryption` / `https-config` / `intelligent-tiering` / `inventory` / `lifecycle` / `logging` / `max-age` / `mirror` / `notification` / `pay-by-traffic` / `payment` / `policy` / `real-time-log` / `rename` / `replication` / `tagging` / `transfer-acceleration` / `trash` / `versioning` / `website` / `worm` | `tos/test_bucket_config_matrix.py` / `tos/test_generated_surface_matrix.py`                                                                               | Handwritten config/read-flow cases cover representative set/get/list/delete behavior; generated execution and parameter guards prevent silent drift across the larger bucket-config surface                 |
| `OBJ` | `object` / `multipart` / `turbo`                                                                                                                                                                                                                                                                                                                                                            | `tos/test_object_lifecycle.py` / `tos/test_core_object_matrix.py` / `tos/test_multipart_turbo_matrix.py` / `tos/test_generated_surface_matrix.py`         | Real object round-trip validates bytes on live TOS; handwritten dry-run matrices cover low-level object and multipart/turbo parameters; generated coverage keeps every object-family leaf command reachable |
| `CTL` | `accelerator` / `ap` / `cap` / `control` / `dataset` / `mrap`                                                                                                                                                                                                                                                                                                                               | `tos/test_advanced_group_matrix.py` / `tos/test_generated_surface_matrix.py`                                                                              | Generic-args handwritten cases validate representative control-plane parsing; generated coverage extends reachability across all control-plane leaves discovered from capabilities                          |
| `DP`  | `data-process`                                                                                                                                                                                                                                                                                                                                                                              | `tos/test_advanced_group_matrix.py` / `tos/test_generated_surface_matrix.py`                                                                              | Generic-args handwritten cases validate representative data-process parsing; generated coverage keeps the wider data-process surface in sync with registry metadata                                         |
| `OS`  | `object-set`                                                                                                                                                                                                                                                                                                                                                                                | `tos/test_advanced_group_matrix.py` / `tos/test_generated_surface_matrix.py`                                                                              | Generic-args handwritten cases validate representative object-set parsing; generated coverage ensures all object-set leaves remain wired through the CLI                                                    |
| `CFG` | `config`                                                                                                                                                                                                                                                                                                                                                                                    | `tos/test_utilities_config_api.py`                                                                                                                        | Real config flow validates `init -> set -> show` with isolated `HOME`, redaction checks, and utility-scope restrictions                                                                                     |
| `AGT` | `api` / `completion` / `serve` / `skill` / `doctor` / `capabilities`                                                                                                                                                                                                                                                                                                                         | `tos/test_utilities_config_api.py` / `tos/test_utilities_agent_audit.py` / `adrive/test_utilities_agent_audit.py`                                        | Utility tests validate `api --describe`, guarded API execution, registry-backed completion generation, skill metadata, MCP serve dry-run, and doctor principle/MCP checks                                    |

Notes:

- `completion`, `serve`, `doctor`, `skill`, and `capabilities` are covered by
  utility agent-audit tests because they validate registry/MCP contracts rather
  than live object-storage data paths.
- `test_parameter_surface.py` is the group-agnostic guardrail that enforces parameter coverage across all in-scope
  groups.
- `test_generated_surface_matrix.py` is the group-agnostic reachability audit that enforces leaf-command coverage across
  all in-scope non-utility groups.

## Environment

Real-resource E2E uses only these environment variables:

```bash
export TOS_ACCESS_KEY=<your-tos-access-key-id>
export TOS_SECRET_KEY=<your-tos-secret-access-key>
export TOS_ENDPOINT=https://tos-cn-beijing.volces.com
export TOS_REGION=cn-beijing
```

Optional variables:

```bash
# Prefix for temporary buckets created by fixtures
export TOS_E2E_BUCKET_PREFIX=ve-tos-cli-e2e

# Keep temporary buckets for debugging instead of deleting them in teardown
export TOS_E2E_KEEP=1
```

ADrive high-level E2E uses ADrive credentials for dry-run and safety checks:

```bash
export ADRIVE_ACCESS_KEY=<your-adrive-access-key-id>
export ADRIVE_SECRET_KEY=<your-adrive-secret-access-key>
export ADRIVE_ENDPOINT=https://...
export ADRIVE_REGION=cn-beijing
```

Live ADrive high-level lifecycle tests create an isolated instance and space with `ve-adrive crt`
by default, then delete the created instance with `ve-adrive del` during teardown. To reuse an
existing workspace instead, set:

```bash
export ADRIVE_E2E_INSTANCE=...
export ADRIVE_E2E_SPACE=...
export ADRIVE_E2E_ROOT_PREFIX=tos-uni-adrive-e2e
```

Set `ADRIVE_E2E_KEEP=1` to keep an automatically-created ADrive instance for debugging.
Use `ve-adrive rm adrive://<instance>/<space> --recursive --include-uploads`
when cleaning an existing space that may contain unfinished multipart uploads
recorded in this CLI's local ADrive checkpoints. Add `--checkpoint-dir <dir>`
when the upload used a custom checkpoint directory.

## Setup

```bash
# 1. Build the binary under test
cargo build --release

# 2. Create an isolated Python environment
cd scripts/e2e
python3 -m venv .venv
source .venv/bin/activate
pip install -e .
```

## Running Tests

```bash
# Run the full suite
pytest -v

# Run metadata / dry-run / non-destructive checks only
pytest -v -m "not destructive and not live"

# Run tests that require valid TOS credentials or mutate real resources
pytest -v -m "destructive or live"

# Run a specific module
pytest -v tos/test_generated_surface_matrix.py
pytest -v tos/test_envelope_schema.py

# Skip the generated slow matrix locally
pytest -v -m "not slow"
```

## TOS protocol validation

Run these focused checks from the repository root. Local HTTP fixtures validate
request placement and state readback without accessing a real service:

```bash
cargo test -p ve-storage-uni-cli --test tos_protocol_http_test \
  --locked --offline -- --test-threads=1
```

The protocol tests cover these contracts:

| Command family | Required request contract |
| --- | --- |
| object set-meta | POST ?metadata; encoded x-tos-meta-* headers; no JSON body; explicit x-tos-metadata-directive REPLACE_NEW (default) or REPLACE; Content-Type only when requested |
| object set-time | POST ?time; x-tos-modify-timestamp Unix seconds and x-tos-modify-timestamp-ns fractional nanoseconds; no JSON body |
| object set-expires | POST ?objectExpires; JSON ObjectExpires integer TTL days; zero clears expiration |
| object fetch | POST ?fetch; URL in JSON; metadata and storage class in headers |
| object set-tagging | PUT ?tagging; JSON TagSet.Tags containing Key/Value entries |
| Version-specific object APIs | versionId query, preserving isolation from the current version |
| object batch-delete | resolved query keys recursive and skipTrash, not Go constant identifiers |
| object status | GetFileStatus alias, GET ?stat |
| data-process jobs | nonempty job_type query; get-job uses --job-id (legacy --id supported) |
| object-set named operations | ObjectSetName=name, not name as a query key |
| dataset binding | DELETE /datasetbinding with DatasetName/URI JSON; GET/list use datasetname and uri query selectors |

A successful write response alone is insufficient. Readback must verify metadata
merge/replacement, literal percent values, Content-Type preservation and explicit
overrides, tags, TTL set/clear, and historical-version isolation. Public expiry
readback uses `x-tos-expiration`; `x-tos-object-expires` is not the public HEAD
readback contract. The local fixture tests live in
[`tests/tos_protocol_http_test.rs`](../../tests/tos_protocol_http_test.rs).

For real-service readback, configure the TOS credentials above, build the unified
binary under test, and explicitly opt in. This command creates and cleans up
disposable buckets; run it only against an approved test account:

```bash
TOS_E2E_PROTOCOL_BIN="$PWD/target/release/ve-storage-uni-cli" \
TOS_E2E_PROTOCOL_LIVE=1 \
  python3 -m pytest scripts/e2e/tos/test_object_protocol_readback.py -v
```

`TOS_E2E_PROTOCOL_BIN` selects the absolute path of the tested binary; adjust it
if the build is elsewhere. Fetch additionally requires
`TOS_E2E_PROTOCOL_FETCH_URL`, a small source accessible by TOS, and is skipped
when that value is absent. SetObjectTime needs a service-enabled HNS test
account. Bucket-creation preconditions can skip a fixture; unsupported API
capabilities after bucket creation remain visible failures. Version isolation
waits for versioning configuration and version visibility before mutating an old
version, then cleans up all versions. These are acceptance requirements, not a
claim that a particular environment has passed them.

Protocol reference sources:

- [Official TOS SDK request types](https://github.com/volcengine/ve-tos-golang-sdk/blob/main/tos/type.go)
- [Official object request construction](https://github.com/volcengine/ve-tos-golang-sdk/blob/main/tos/object.go)
- [Official Fetch requests](https://github.com/volcengine/ve-tos-golang-sdk/blob/main/tos/fetch.go)
- [Official dataset binding requests](https://github.com/volcengine/ve-tos-js-sdk/blob/main/src/methods/datasetSearch/binding.ts)
- [ObjectSet lifecycle selector](https://docs.volcengine.com/docs/torch-object-storage/delete-object-set-life-cycle?lang=zh)
- [Job query parameters](https://docs.volcengine.com/docs/torch-object-storage/query-media-job?lang=zh)

## Resource Cleanup

- Temporary buckets use a unique 16-hex suffix per test session to avoid collisions.
- The `temp_bucket` fixture always performs teardown in a `finally` block.
- Cleanup is best-effort and follows a strict order:
  `ve-tos rm tos://bucket/ --recursive --force --confirm tos://bucket/` ->
  `ve-tos bucket delete --force --confirm tos://bucket`
- Cleanup failures are logged to `stderr` as warnings instead of masking the original test result.
- All real write-path tests must use `temp_bucket` or `fresh_bucket_name`, or must implement an explicit `finally`
  cleanup path.

## Exit Code Contract

The suite validates the exit-code mapping defined in
`crates/tos-core/src/agent/error.rs::ExitCode`:

`Success=0 / Unknown=1 / AuthFailed=2 / ConfigMissing=3 / ResourceNotFound=4 / PermissionDenied=5 / ValidationError=6 / RateLimited=7 / TransferFailed=8 / Conflict=9`

## Design Notes

- No `boto3`: E2E must validate our own CLI contract, not an SDK shortcut.
- `subprocess.run()` over direct imports: the binary is the product surface that users and agents invoke.
- Streaming verification stays lightweight: the suite validates representative payload sizes, not GiB-scale performance
  workloads.
- Registry metadata is the SSOT for generated command coverage and parameter-surface audits.
