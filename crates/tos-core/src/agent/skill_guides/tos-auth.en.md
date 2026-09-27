## ByteCloud TOS authentication

`aksk` is the default. Select ZTI with `--auth-mode zti`, or set
`[profile.tos].auth_mode = "zti"` in the profile. Resolution order is the CLI flag,
profile, `BYTETOS_AUTH_MODE`, then `aksk`. ZTI reads a token from
`SEC_TOKEN_STRING`, a local Agent at `ZTI_AGENT_SOCKET_PATH` (default
`/run/zti-agent.sock`), or `SEC_TOKEN_PATH`, in that priority order. The Agent
source is unavailable on Windows. The file source is checked for changes on
each credential request; the Agent source refreshes before expiration or at
most every 600 seconds. ZTI does not use AK/SK and never falls back to it.

Run `doctor --check auth` to inspect the selected mode and configured source;
that offline check does not read the token or verify remote access. Keep token
values out of CLI arguments, MCP arguments, logs, and exported skills. ZTI does
not support presign; use `aksk` for a signed URL only when the user has
authorized that authentication mode.
