# Explicit Service Endpoint Design

## Goal

Make service-address configuration explicit. The CLI may derive a signing
region from a recognizable endpoint, but it must not invent an endpoint from a
region. This avoids assuming that every region uses a public-domain naming
template and keeps private, test, proxy, and PSM deployments correct.

The rule applies to the `tos`, `ve-tos`, and `ve-adrive` surfaces.

## Configuration contract

### `ve-tos`

`ve-tos config init` continues to write the known production pair:

```toml
[default]
region = "cn-beijing"

[default.ve-tos]
endpoint = "tos-cn-beijing.volces.com"
```

Outside that initialization default, runtime code does not construct an
endpoint from region. An explicitly configured recognizable endpoint may still
supply region when region is absent. A custom endpoint whose region cannot be
parsed requires an explicit region.

### `tos`

`tos config init` creates the shared profile and `[profile.tos]` section with
the existing non-network operational settings, but does not write `region`,
`endpoint`, or `psm`.

Endpoint mode requires an explicit endpoint. Region may be explicit or parsed
from a recognizable endpoint. PSM mode continues to be mutually exclusive with
endpoint mode and requires the user to configure both region and PSM. PSM is
never inferred.

### `ve-adrive`

`ve-adrive config init` continues to omit resource region and endpoint. Resource
requests require an explicit endpoint; region may be explicit or parsed from a
recognizable IDS endpoint.

OAuth login requires an explicit Authorization Server `auth_endpoint` from the
command line, selected profile, or `ADRIVE_AUTH_ENDPOINT`. The built-in
`https://idsauth.volces.com` fallback is removed. Resource endpoint and OAuth
auth endpoint remain independent settings.

Existing OAuth credentials that already store their resolved auth endpoint
remain usable for refresh. The change affects new login attempts that do not
provide an auth endpoint.

## Resolution and precedence

Existing source precedence remains unchanged:

```text
command-line option > selected config profile > environment variable
```

After precedence resolution:

1. Use the explicit endpoint when present. If it has no URL scheme, prefix
   `https://`; this includes dotless service-discovery hosts with numeric ports
   such as `resource:9000`.
2. Use the explicit region when present.
3. If region is absent, attempt to parse it from the resolved endpoint.
4. If endpoint is absent, return an actionable configuration error; never
   construct an endpoint from region.
5. If region is still absent, return an actionable configuration error.
6. Reject ADrive resource endpoints whose resolved URL is not HTTP(S) or has no
   host before request signing and construction.

No mapping table or string-template fallback is introduced.

## Compatibility

The following behavior remains compatible:

- configurations containing both endpoint and region;
- configurations containing a recognizable endpoint but no region;
- the `ve-tos config init` production defaults;
- TOS PSM configurations that already contain region and PSM;
- refresh using OAuth credentials that contain a stored auth endpoint.

The following formerly accepted behavior intentionally becomes invalid:

- configuring only region and relying on the CLI to construct a TOS or IDS
  endpoint;
- starting a new ADrive OAuth login without an explicitly supplied
  `auth_endpoint`;
- using the network defaults previously written by `tos config init` without
  replacing them with user-selected settings.

Existing configuration values are never migrated, deleted, or rewritten.

## Errors and diagnostics

Missing configuration errors identify the exact field and accepted sources.
They must not recommend unrelated authentication actions:

- missing resource endpoint: recommend `config set endpoint`, `--endpoint`, or
  the surface-specific endpoint environment variable;
- unparseable/missing region: recommend `config set region`, `--region`, or the
  surface-specific region environment variable;
- missing OAuth auth endpoint: recommend `ve-adrive config set auth_endpoint`,
  `ve-adrive auth login --auth-endpoint`, or `ADRIVE_AUTH_ENDPOINT`;
- missing PSM connection data: identify region and PSM independently.

`config show` and Doctor continue to expose each resolved value and source;
region parsed from endpoint remains marked `Derived`.

## Documentation and verification

The behavior must be synchronized across command help, `--describe`, Doctor,
and applicable `SKILL.md` guidance.

Regression coverage must verify at least:

1. `ve-tos config init` still writes the Beijing region and endpoint.
2. `tos config init` writes neither region, endpoint, nor PSM.
3. Every resource client rejects region-only configuration instead of building
   an endpoint.
4. Recognizable endpoint-only configurations still derive region.
5. Custom endpoints require an explicit region.
6. TOS PSM mode accepts explicit region plus PSM without endpoint.
7. ADrive OAuth login rejects a missing auth endpoint with actionable guidance.
8. Existing stored OAuth auth endpoints remain usable for refresh.
9. Explicit config, CLI, and environment precedence remains unchanged.
