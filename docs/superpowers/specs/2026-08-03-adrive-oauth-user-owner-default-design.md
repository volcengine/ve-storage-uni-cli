# ADrive OAuth User Owner Default Design

## Goal

Allow OAuth users to create a user-owned Space without repeating
`--owner-id`, using the `user_id` returned by the OAuth Token endpoint, while
keeping group ownership explicit and keeping AK/SK behavior unchanged.

## Credential representation

`user_id` is optional OAuth issuer metadata and is stored in the existing
credentials file under the selected profile:

```toml
schema_version = 1

[default.adrive.oauth]
access_token = "ENC[...]"
refresh_token = "ENC[...]"
expires_at = "2026-08-03T12:00:00Z"
token_type = "Bearer"
scope = ["all"]
instance_id = "instance-id"
auth_endpoint = "https://auth.example.com"
user_id = "user-id"
```

The field is added to `StoredOAuthCredentials` as `Option<String>`. Existing
credentials without it remain readable, and the credentials schema version
remains `1`. Like `instance_id`, `scope`, and `auth_endpoint`, `user_id` is
metadata rather than a bearer credential, so it is not encrypted. The entire
credentials file continues to use owner-only permissions and atomic
replacement. `config show` must not expose the actual value.

The CLI does not decode `access_token` to recover this value and does not
create a separate identity cache.

## Login and refresh lifecycle

On successful device login, a non-empty Token response `user_id` is saved with
the access token, refresh token, Instance binding, and authorization endpoint.
An absent or blank `user_id` does not make login fail.

When refreshing credentials:

- a non-empty `user_id` in the Refresh response replaces the saved value;
- an absent or blank Refresh response `user_id` preserves the saved value;
- all existing token validation and invalid-grant clearing behavior remains
  unchanged.

This preservation rule is required because a Refresh response may omit
identity metadata even when the original device-login response supplied it.

## Space owner resolution

Space creation resolves the effective owner type before constructing the
request:

- AK/SK keeps both owner fields optional and sends only explicitly supplied
  values.
- OAuth defaults an omitted `--owner-type` to `user`.
- An explicit `--owner-id` always wins.
- OAuth with effective owner type `user` and no explicit owner ID uses the
  non-empty `user_id` stored with the selected OAuth credentials.
- OAuth with owner type `group` always requires an explicit `--owner-id`.
- If OAuth user ownership cannot obtain an ID from either source, validation
  fails before Resource or name-resolution requests. The error directs the
  caller to provide `--owner-id` or rerun `ve-adrive auth login`.

Environment-only OAuth credentials have no Token-response metadata. They can
create a Space only when `--owner-id` is supplied explicitly.

Dry-run and real execution use the same owner-resolution function so they
produce identical request bodies and validation errors.

## Discovery and documentation synchronization

The implementation updates all applicable user-visible surfaces together:

- `ve-adrive crt --help` explains OAuth user defaulting, the group requirement,
  and includes examples for both paths.
- `ve-adrive crt --describe` describes the conditional `owner-id` requirement
  rather than claiming it is always required in OAuth mode.
- `skills/ve-adrive-cli/SKILL.md` documents OAuth login, owner-aware Space
  creation, and OAuth user/group Space listing.
- The earlier ADrive create/list design is corrected so it no longer states
  that every OAuth Space creation requires `--owner-id`.
- Registry, help, skill, credential lifecycle, validation, dry-run, and
  backward-reading tests cover the new contract.

The general requirement to synchronize `--help`, `--describe`, installable
skills, documentation, and tests for every user-visible feature is recorded in
`docs/development_workflow.md`.

## Compatibility and non-goals

- No AK/SK precedence, request signing, or request body default changes.
- No authentication-mode precedence changes.
- No automatic migration and no credential double-write.
- Old credentials remain usable; they simply cannot supply the new owner
  default until the user logs in again or explicitly passes `--owner-id`.
- Token introspection and a current-user Resource API are outside this scope.

## Verification

Tests cover at least:

1. Device login persists a non-empty `user_id` and old credentials without the
   field still load.
2. Refresh replaces `user_id` when returned and preserves it when omitted.
3. OAuth user ownership uses stored `user_id`, including when owner type is
   omitted.
4. Explicit `--owner-id` overrides the stored OAuth user ID.
5. OAuth group ownership without `--owner-id` fails before any request.
6. Missing OAuth user metadata produces the targeted login-or-owner-id error.
7. AK/SK request bodies remain unchanged.
8. Dry-run and real execution resolve identical ownership fields.
9. Help, describe, and installable skill text expose the new behavior.
