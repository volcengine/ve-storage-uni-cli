# Auth Chinese Documentation Design

## Goal

Complete Chinese documentation for the newly added ADrive OAuth and Unified
authentication capabilities, plus VeTos Unified authentication, without
changing command behavior or translating unrelated historical content.

## Scope

The change covers exactly three documentation surfaces:

1. `--help --language zh` for authentication commands and options.
2. `--describe --language zh` for authentication metadata.
3. `skill export --language zh` for generated authentication Skill content.

The repository's checked-in English `skills/ve-adrive-cli/SKILL.md` and
`skills/ve-tos-cli/SKILL.md` remain English because those files do not expose a
language selector. Existing non-authentication English fragments in Chinese
output are outside this change.

## Help contract

Chinese help must translate the new authentication content while retaining all
stable command and configuration tokens. This includes:

- ADrive `auth`, `status`, `login`, and `logout` descriptions;
- ADrive `--auth-mode`, `--instance`, `--auth-endpoint`, and `--device-name`;
- `aksk`, `oauth`, and `unified` value descriptions;
- VeTos `--auth-mode` and its `aksk` and `unified` value descriptions;
- Unified external-login guidance using the literal commands `ve login` and
  `ve logout`.

The implementation extends the existing exact-phrase localization path.
English help remains byte-for-byte compatible apart from changes made by Clap
itself. Identifiers such as `ADRIVE_AUTH_MODE`, `TOS_AUTH_MODE`, profile keys,
mode values, flags, and example commands are never translated.

## Describe contract

`--describe --language zh` returns Chinese human-readable authentication
metadata. Stable machine-readable fields remain unchanged, including command
paths, parameter names, schema types, enum literals, risk levels, API names,
and example commands.

The localized fields are limited to human-readable values such as:

- command `description`;
- parameter `description`;
- authentication-specific `scenario_routing` explanations;
- shell guidance strings when they describe the authentication command.

`--describe` without `--language zh`, and `--language en`, continue returning
the existing English metadata.

## Skill export contract

`skill export --language zh` produces genuinely Chinese authentication
descriptions rather than prefixing unchanged English text with Chinese labels.
The ADrive Auth Skill and VeTos/ADrive schemas that expose `auth-mode` use the
same stable identifiers and examples as English output. Only explanatory prose
is localized.

English Skill export is unchanged. Skill export continues to reject existing
target paths and preserves existing dry-run behavior.

## Architecture

Help localization remains in the unified dispatcher because it post-processes
Clap output for every binary surface. Describe and Skill localization remain in
their owning ADrive and VeTos metadata modules. Each owner uses exact English to
Chinese mappings for the frozen authentication phrases, avoiding a broad
internationalization refactor and preventing accidental translation of
machine-readable values.

## Validation

Regression tests must prove:

1. ADrive Auth and login Chinese help contains the expected Chinese text and no
   longer contains the corresponding authentication prose in English.
2. VeTos Chinese help translates Unified authentication option/value prose.
3. Chinese ADrive Auth describe output localizes descriptions while preserving
   parameter names, mode literals, environment-variable names, and examples.
4. Chinese generated Auth Skills contain Chinese descriptions and schemas;
   English exports remain unchanged.
5. Existing English help, describe, and Skill tests continue to pass.

No runtime authentication, credential loading, request signing, Doctor, or
service API behavior changes are authorized by this design.
