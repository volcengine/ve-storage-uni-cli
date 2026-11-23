//! Pure, shared validation for publisher build inputs and CLI application selection.

/// Fixed publisher permissions: session creation, per-request reply stream and token renewal.
/// Verified 2026-11-22 against the official session/create, session-event/stream and token docs.
pub(crate) const ASK_SCOPES: &str = "doubao:session:write doubao:session.event:read offline_access";

const MAX_APP_ID_BYTES: usize = 256;
const MAX_SECRET_BYTES: usize = 4096;
const MAX_SCOPES_BYTES: usize = 4096;
const MAX_SCOPE_BYTES: usize = 256;
const MAX_SCOPES: usize = 64;

/// Borrowed, validated application inputs; deliberately has no formatting or serialization traits.
pub(crate) struct ApplicationValues<'a> {
    pub(crate) app_id: &'a str,
    pub(crate) secret: &'a str,
}

/// A value-free validation error suitable for redacted build and runtime diagnostics.
#[derive(Debug)]
pub(crate) struct InvalidApplication;

/// Validate an optional publisher credential pair and the fixed ask permissions.
/// Returns a borrowed bundle, or a value-free error for partial, empty or invalid inputs.
pub(crate) fn parse_bundle<'a>(
    app_id: Option<&'a str>,
    secret: Option<&'a str>,
) -> Result<Option<ApplicationValues<'a>>, InvalidApplication> {
    let (app_id, secret) = match (app_id, secret) {
        (None, None) => return Ok(None),
        (Some(app_id), Some(secret)) => (app_id, secret),
        _ => return Err(InvalidApplication),
    };
    let bundle = ApplicationValues { app_id, secret };
    if !valid_app_id(bundle.app_id)
        || bundle.secret.is_empty()
        || bundle.secret.len() > MAX_SECRET_BYTES
        || !bundle.secret.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(InvalidApplication);
    }
    validate_scopes(ASK_SCOPES)?;
    Ok(Some(bundle))
}

/// Check the bounded ASCII application identifier syntax without allocating or exposing values.
pub(crate) fn valid_app_id(app_id: &str) -> bool {
    !app_id.is_empty()
        && app_id.len() <= MAX_APP_ID_BYTES
        && app_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

/// Validate strict RFC 6749 scope-token bytes and their per-token size limit.
pub(crate) fn valid_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= MAX_SCOPE_BYTES
        && scope
            .bytes()
            .all(|byte| matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e))
}

/// Parse bounded, space-delimited business scopes, rejecting duplicates and offline-only lists.
/// Returns borrowed tokens or a value-free error; no platform scope names are inferred.
pub(crate) fn validate_scopes(scopes: &str) -> Result<Vec<&str>, InvalidApplication> {
    if scopes.is_empty() || scopes.len() > MAX_SCOPES_BYTES {
        return Err(InvalidApplication);
    }
    let mut required = Vec::new();
    for scope in scopes.split(' ') {
        if required.len() >= MAX_SCOPES || !valid_scope_token(scope) || required.contains(&scope) {
            return Err(InvalidApplication);
        }
        required.push(scope);
    }
    if required.iter().all(|scope| *scope == "offline_access") {
        return Err(InvalidApplication);
    }
    Ok(required)
}

#[cfg(test)]
mod tests;
