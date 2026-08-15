# ADrive Create Parameters and OAuth Space Listing Design

Date: 2026-08-03

## Goal

Extend only `ve-adrive-cli` with authentication-aware defaults for Instance/Space creation, and use the IDS user/group Space listing APIs in OAuth mode. Existing AK/SK behavior remains unchanged except for the explicitly documented Instance default.

## CLI Contract

### `ve-adrive crt`

- `--service-type` is valid only when creating an Instance and accepts `saas`, `paas`, or `arkclaw`.
- If `--service-type` is omitted, AK/SK sends `ServiceType=arkclaw`; OAuth sends `ServiceType=paas`.
- `--owner-type` and `--owner-id` are valid only when creating a Space.
- `--owner-type` accepts only the lower-case values `user` and `group`.
- In AK/SK mode, omitted owner fields are omitted from the request body.
- In OAuth mode, omitted `--owner-type` becomes `user`. For effective owner type `user`, an omitted `--owner-id` is filled from the `user_id` persisted from the OAuth Token response. For owner type `group`, `--owner-id` remains required. If neither an explicit owner ID nor stored OAuth `user_id` is available for user ownership, validation fails before a request is sent.

The request structs continue using IDS PascalCase field names. Therefore the new values are serialized as `ServiceType`, `OwnerType`, and `OwnerId`.

### `ve-adrive ls adrive://<instance>`

- AK/SK keeps calling `GET /v1/instances/{instance_id}/spaces`.
- OAuth with omitted `--owner-type`, or with `--owner-type user`, calls `GET /v1/instances/{instance_id}/myspaces`.
- OAuth with `--owner-type group` calls `GET /v1/instances/{instance_id}/mygroupspaces`.
- AK/SK rejects `--owner-type` instead of silently ignoring it.
- `--owner-type` is rejected when listing Instances or files because it only selects an OAuth Space collection.

Both OAuth APIs use only `limit` and `marker` query parameters. Their continuation condition is a non-empty `NextMarker`; unlike the existing `list_spaces` response, the SDK contract does not include `IsTruncated`.

`list_my_group_spaces` additionally returns an optional `RootSpace`. The CLI preserves it as a separate `root_space` response field and does not merge it into `spaces`, pagination counts, or manifests.

## Authentication Boundary

The Resource client exposes a read-only authentication-kind query to the high-level handler. API routing and create defaults use the actual provider already selected by existing precedence rules. No auth-mode resolution or credential precedence changes are introduced.

The client implements three explicit listing methods. The existing `list_spaces` method never changes its endpoint internally; this keeps AK/SK semantics stable and makes OAuth endpoint selection testable.

## OAuth Login Output

Device login prints `verification_uri_complete` and the waiting status only. QR generation and the fallback verification URI/user code line are removed. Authorization polling and token persistence are unchanged.

## Validation and Errors

All invalid combinations fail before network I/O:

- Instance creation with owner options.
- Space creation with `--service-type`.
- OAuth group Space creation without `--owner-id`, or OAuth user Space creation without either `--owner-id` or persisted Token-response `user_id`.
- `--owner-type` outside `user|group`.
- `ls --owner-type` outside OAuth instance-level Space listing.

Errors remain `CliError::ValidationError`, preserving the current structured error envelope and exit-code behavior.

## Compatibility

- No behavior changes are made to `ve-tos-cli` or `tos-cli`.
- Existing AK/SK Space listing and pagination are untouched.
- Existing AK/SK Space creation payloads remain unchanged when owner options are omitted.
- Existing URI and option forms remain accepted.
- OAuth token refresh, retries, credentials storage, and Resource endpoint handling are unchanged.

## Verification

Tests cover CLI enum parsing, request serialization and omission, mode-aware defaults, invalid option combinations, exact OAuth paths/query parameters, OAuth pagination without `IsTruncated`, group `RootSpace` preservation, AK/SK routing compatibility, and URL-only login output.
