# CLI v1.0.4 release preparation

Status: source and metadata prepared for review; packages and release assets have not been uploaded by this change.

## Version scope

- Public install names: `ve-tos-cli`, `tos-cli`, `ve-adrive-cli`, all 1.0.4.
- Root dispatcher and CLI workspace crates, including `tos-core`: 1.0.4, with matching local dependency requirements and lockfiles.
- Vector/Table scaffolds: version-aligned only; excluded from the current public publishing pipeline.
- ask-auth: independent service version, excluded from the CLI release version.

The previous ADrive-only release preparation kept shared packages at 1.0.3. Subsequent shared fixes now also change tos-cli behavior, so this coordinated release is justified by user-visible fixes rather than version alignment alone.

## Changes by CLI

### tos-cli

Public Help/Usage/examples and CLI-authored errors now use the executable actually invoked. Parse errors, fix_command, doctor_hint, batch failure summaries, configuration validation, Doctor and generated instructions no longer direct ByteTOS users to the wrong entry point. Canonical machine command IDs and opaque user/server text remain stable. These changes were merged as a384daf, ff9f88e, 3582c7d and 527f5ae. MR96 also corrects Chinese recursive-listing help to advertise hierarchical only; it adds no ByteTOS storage-protocol changes.

### ve-tos-cli

Correct Low-Level metadata/time/expiry/fetch/tagging request placement, object version and batch-delete query names, status routing, job selectors, ObjectSet names and dataset binding requests. Metadata defaults to explicit REPLACE_NEW; REPLACE is available. Content-Type is sent only when explicitly requested. ObjectExpires is an integer TTL in days, with zero clearing expiration; RFC3339 and fractional expiry arguments are rejected. High-Level restore also uses versionId.

### ve-adrive-cli

Includes the already-merged file metadata upload/update and command/MCP contract work from MR94, plus preceding shared instruction and diagnostic fixes. The existing 1.0.4 source version is retained.

## Verification and remaining gates

Protocol fixes have 406 passing TOS core checks, 13 local HTTP request/readback checks, and four real-service readback cases (metadata, tagging, TTL set/clear and historical-version isolation). Real SetObjectTime is blocked by the test bucket capability; Fetch has not been tested against a live source URL. The three previously recorded master test failures are addressed by correcting two test preconditions/contracts and the Chinese help filter; the detailed verification record is in `docs/tos-protocol-corrections-v1.0.4.md`.

Release metadata, dedicated entrypoints and package generators must be rechecked after the version-only follow-up. Publishing uses the final reviewed source revision and native CI runners for all supported platforms. Keep a clean build output directory and validate binary versions, archive membership and checksums.

The prior ADrive-only 1.0.4 may already exist in a registry. Verify each target channel before publishing: existing package/version artifacts and tag targets are not overwritten. If that version is occupied, newly built ADrive code needs a new version in that channel. This preparation neither publishes nor asserts availability of a package version.

Preparation checks after version alignment: workspace `cargo check --locked --offline` passed; all 132 packaging tests and all 16 dedicated-entry tests passed. All ten CLI package versions and their local dependency requirements were checked against the four lockfiles. npm wrapper trees for all three install names were generated at 1.0.4; the publication preview still excludes the occupied npm tos-cli name. Cargo publishing was previewed in dependency order. No registry upload, GitHub Release creation, tag movement or deployment was executed.
