# TOS Low-Level protocol corrections (1.0.4)

This change follows the external TOS wire protocol, not the automatically extracted server-handler index. The index omits dynamic headers, mistakes constant identifiers for query names, and can report empty request bodies when the parser is outside its limited call graph.

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

## Protocol evidence

- [Official TOS SDK request types](https://github.com/volcengine/ve-tos-golang-sdk/blob/main/tos/type.go)
- [Official object request construction](https://github.com/volcengine/ve-tos-golang-sdk/blob/main/tos/object.go)
- [Official Fetch requests](https://github.com/volcengine/ve-tos-golang-sdk/blob/main/tos/fetch.go)
- [Official dataset binding requests](https://github.com/volcengine/ve-tos-js-sdk/blob/main/src/methods/datasetSearch/binding.ts)
- [ObjectSet lifecycle selector](https://docs.volcengine.com/docs/torch-object-storage/delete-object-set-life-cycle?lang=zh)
- [Job query parameters](https://docs.volcengine.com/docs/torch-object-storage/query-media-job?lang=zh)

The available server checkout additionally confirms `queryVersionID = "versionId"` in tosapi/api_handler.go, x-tos-* header normalization in middleware.go, and SetObjectMeta/SetObjectTime/SetObjectExpires request parsing in commercial_broker.go. The CLI does not depend on that checkout for build or tests.

## Validation

`tests/tos_protocol_http_test.rs` exercises the real CLI against a bounded local HTTP fixture. Assertions cover request placement and state readback, including unchanged content type, merge/replacement, literal percent characters, version isolation and resource selectors. Invalid inputs must fail before requests are sent. A successful write response alone is insufficient.

Real-service tests are explicitly opt-in and use disposable buckets:

```sh
TOS_E2E_PROTOCOL_LIVE=1 python3 -m pytest scripts/e2e/tos/test_object_protocol_readback.py -v
```

Set the standard TOS E2E credential variables and `TOS_E2E_PROTOCOL_BIN` as described in scripts/e2e/README.md. Fetch also needs TOS_E2E_PROTOCOL_FETCH_URL. Time mutation needs a service-enabled HNS test account. Once a test bucket exists, unsupported capabilities remain visible failures when execution is requested. Shared fixture setup can skip bucket creation failures. Public expiration readback parses x-tos-expiration and verifies clearing the TTL; raw x-tos-object-expires is not a public readback contract.

## Release scope

The three dedicated CLIs, root dispatcher and all CLI workspace crates (including tos-core) use 1.0.4. The ask-auth service has an independent version. Vector/Table scaffold versions are aligned but are not added to the public publishing pipeline. ByteTOS has independent dispatch and does not expose these ve-tos Low-Level commands; this MR adds no ByteTOS storage-protocol changes. The 1.0.4 tos-cli release nevertheless includes the preceding shared Help/Usage, error-message, remediation and Doctor fixes from a384daf, ff9f88e, 3582c7d and 527f5ae. Shared dependency identities and lockfiles are updated without changing third-party dependency versions. MR94 is already in master and is not modified by this branch.

## Review and verification record

Four independent review passes covered the complete patch and later TTL/batch-delete corrections. The review initially missed the expiry unit; backend consumers and real HEAD readback corrected the assumption to TTL days. Tag validation and exact Chinese-help catalog matches were also tightened. No unresolved Critical/Major issue was identified in the final production diff.

Final focused checks: TOS core 406/406; unified-entry library 24/24; CLI parsing/help 186/186; core contracts 16/16; loopback wire/readback 13/13; dedicated entries 16/16; packaging 132/132. Formatting and whitespace checks pass. These suites overlap; their counts are not a combined unique-test total.

A complete serial workspace run preceded the last localization/test updates: 2135 passed, 8 failed, 102 ignored. Five localization failures were corrected and their suites rerun successfully. Three additional failures were also recorded against master in MR94's verification notes; the two help behaviors were reproduced with the preserved baseline binary. The follow-up now closes all three:

- `advanced_principles_test::test_advanced_body_commands_require_config_for_execution_paths`: explicitly supply region so missing-config validation is tested independently of host configuration. The existing region-before-config validation order is retained.
- The former `functional_agent_contract_test::adrive_root_help_lists_all_share_actions_in_both_languages` is renamed to `adrive_share_help_lists_all_actions_in_both_languages`: inspect all nine actions in share-group help. Existing root-layer tests continue to require a share namespace rather than expanded leaf commands.
- `tos_auth_mode_test::tos_recursive_list_help_only_offers_hierarchical`: correct both the Chinese description and localized inline possible-values list. Cover cp/mv/sync/rm in both languages with long and short help; ve-tos retains all three supported modes.

Follow-up verification: 263/263 checks pass across unified-entry library (24), advanced principles (10), CLI parsing/help (186), functional agent contracts (11), metadata consistency (14) and ByteTOS auth/help (18). The run clears host region/config overrides; local fixture servers and PTY tests require permission to bind loopback ports. An additional 16 ve-tos help surfaces retain auto/flat/hierarchical. Formatting and whitespace checks pass. Review found the localized short-help enum omission during the first correction pass; it was fixed and the complete focused suites rerun, with no unresolved Critical/Major issue. The entire workspace has not been rerun after this follow-up, so these results are not a new all-workspace green claim. An initial parallel workspace run additionally exposed existing process-environment test races; all eight affected checks passed individually/serially.

Final real-service run: 4 passed (metadata, tagging, TTL set/clear and version isolation). Real-service readback verifies metadata merge/replace, literal percent values, Content-Type preservation/override, tagging and TTL set/clear in disposable buckets. Version isolation additionally waits for new-bucket versioning configuration and version visibility before modifying an old version. SetObjectTime is rejected by this test account's bucket gate (HTTP 405 NotSupportSetObjectTime); Fetch requires an explicitly supplied accessible source URL. These two cloud checks are not claimed as passing. Test resources are cleaned up after execution.
