# Unified Credentials Store Design

## Scope

Introduce `credentials.toml` for sensitive credentials used by `tos-cli`,
`ve-tos-cli`, and `ve-adrive-cli`. Only `ve-adrive-cli` supports selecting an
authentication mode (`aksk` or `oauth`). No OAuth server endpoint is called in
this phase.

The change must preserve existing runtime behavior. Credentials already stored
in `config.toml` remain readable. The CLI does not migrate them automatically,
delete them, or mirror new writes back to `config.toml`.

## Paths

The normal paths are:

```text
$HOME/.tos/config.toml
$HOME/.tos/credentials.toml
```

`--credentials-path <PATH>` and `TOS_CREDENTIALS_PATH` select an explicit
credentials file. Without either override, the credentials file is named
`credentials.toml` in the directory containing the effective config file.
Therefore `--config-path /data/cli/config.toml` implies
`/data/cli/credentials.toml` unless the credentials path is explicitly set.

Path precedence is:

```text
--credentials-path > TOS_CREDENTIALS_PATH > sibling of effective config path
```

## File schema

```toml
schema_version = 1

[default]
access_key_id = "ENC:..."
secret_access_key = "ENC:..."
security_token = "ENC:..."

[default.tos]
access_key_id = "ENC:..."
secret_access_key = "ENC:..."
security_token = "ENC:..."

[default.ve-tos]
access_key_id = "ENC:..."
secret_access_key = "ENC:..."
security_token = "ENC:..."

[default.adrive]
access_key_id = "ENC:..."
secret_access_key = "ENC:..."
security_token = "ENC:..."

[default.adrive.oauth]
access_token = "ENC:..."
refresh_token = "ENC:..."
expires_at = "2026-07-11T12:00:00Z"
token_type = "Bearer"
scope = ["example.scope"]
```

The credentials file deliberately mirrors the profile hierarchy in
`config.toml`: root profile credentials are shared by `tos-cli` and
`ve-tos-cli`, while a surface-specific section overrides them. ADrive never
inherits root profile credentials. OAuth is the only extra child section
because it is a separate credential family owned by ADrive.

`schema_version` is a reserved top-level key and cannot be used as a profile
name. The previously implemented `[profiles.<name>.<surface>.<family>]` layout
was never released and is not accepted or migrated; this keeps one canonical
on-disk representation instead of carrying two private formats.

Every secret is encrypted with the existing AES-256-GCM `ENC:` mechanism. The
credentials file and its local key use mode `0600` on Unix. Writes use a
temporary sibling file followed by rename so a failed write cannot truncate the
last valid credentials file.

## Resolution and compatibility

Credential resolution is field-by-field within the selected profile and CLI
surface, matching the existing profile merge behavior:

```text
credentials.toml > legacy config.toml > environment variables
```

This field-level behavior allows a user to set AK and SK with two existing
`config set` invocations without breaking compatibility during the transition.

For ADrive OAuth:

```text
credentials.toml > ADRIVE_ACCESS_TOKEN / ADRIVE_REFRESH_TOKEN
```

ADrive authentication-mode resolution follows the existing CLI configuration
rule:

```text
--auth-mode > config.toml auth_mode > ADRIVE_AUTH_MODE > default aksk
```

Selecting OAuth never falls back to AK/SK, and selecting AK/SK never consumes
OAuth tokens.

## Write behavior

For the credential keys `access_key_id`, `secret_access_key`, and
`security_token`, all three existing `config set` surfaces route new writes to
the corresponding profile and surface in `credentials.toml`: bare keys use
`[profile.tos]`, `[profile.ve-tos]`, or `[profile.adrive]` according to the
active CLI. An explicit two-segment key such as `default.access_key_id` retains
the shared `[default]` write behavior for compatibility; shared root credentials
are inherited only by `tos-cli` and `ve-tos-cli`. Non-sensitive keys continue to
write only to `config.toml`.

OAuth login will eventually write the ADrive OAuth section. In this phase the
store exposes read/write/delete APIs and the auth handlers remain offline
placeholders.

## Inspection behavior

`config show` reads both persistent files. It preserves the existing non-secret
config view, overlays credentials from `credentials.toml`, and displays only
masked credential values. JSON includes both `config_path` and
`credentials_path`; table output identifies credential sources as
`credentials_file` or `legacy_config`.

`auth status` and `doctor` use the same credential resolver as runtime commands.
They report presence and source, never raw secrets.

## Error handling

- A missing credentials file is equivalent to an empty store.
- An explicitly selected missing credentials path is an error for runtime or
  write commands, matching explicit config-path behavior, except that a write
  command may create the file and its parent directory.
- Invalid TOML, unsupported schema versions, and corrupted ciphertext produce
  deterministic validation errors. CLI-created files always use mode `0600`.
- A credentials write never changes `config.toml`; a non-credential config
  write never changes `credentials.toml`.

## Testing

Tests cover path derivation and overrides, encrypted atomic persistence,
surface/profile isolation, new-store precedence, legacy config fallback,
environment fallback, partial-set rejection, no migration/no dual-write,
dual-file `config show`, ADrive mode isolation, and full existing CLI
regressions.
