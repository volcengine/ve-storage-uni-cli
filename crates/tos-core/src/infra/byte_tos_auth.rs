//! Authentication strategies for the ByteCloud TOS entry point.

use clap::ValueEnum;
use serde::Serialize;

use crate::agent::error::CliError;

/// Authentication strategy selected for a ByteCloud TOS invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ByteTosAuthMode {
    /// Sign requests with access-key and secret-key credentials.
    Aksk,
    /// Authenticate using a ByteCloud ZTI token.
    Zti,
}

impl ByteTosAuthMode {
    /// Return the stable configuration and output representation of this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aksk => "aksk",
            Self::Zti => "zti",
        }
    }

    /// Parse `value`, normalizing whitespace and ASCII case, for `source`.
    ///
    /// Returns the selected mode, or [`CliError::ValidationError`] when the
    /// value is neither `aksk` nor `zti`.
    pub fn parse(value: &str, source: &str) -> Result<Self, CliError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "aksk" => Ok(Self::Aksk),
            "zti" => Ok(Self::Zti),
            _ => Err(CliError::ValidationError(format!(
                "invalid tos auth mode '{}' from {}; expected aksk or zti",
                value, source
            ))),
        }
    }
}

/// Return the ByteCloud authentication flag metadata for command descriptions.
///
/// The returned optional flag schema documents accepted modes, resolution
/// priority, and the token sources for ZTI authentication.
pub fn byte_tos_auth_parameter() -> crate::agent::describe::CommandParameter {
    let sources = if cfg!(unix) {
        "SEC_TOKEN_STRING, a local Agent, or SEC_TOKEN_PATH"
    } else {
        "SEC_TOKEN_STRING or SEC_TOKEN_PATH"
    };
    crate::agent::describe::CommandParameter {
        name: "auth_mode".to_string(),
        location: crate::agent::describe::ParameterLocation::Flag,
        required: false,
        description: format!("ByteCloud TOS authentication mode. Priority: --auth-mode > [profile.tos].auth_mode > BYTETOS_AUTH_MODE > aksk. zti uses {sources}, ignores AK/SK, and has no fallback."),
        schema: Some(serde_json::json!({
            "type": "string", "enum": ["aksk", "zti"], "default": "aksk"
        })),
    }
}

/// Return the ByteTOS authentication contract for offline discovery surfaces.
///
/// # Returns
///
/// Supported modes, default and precedence, public ZTI availability, source
/// order, and the presign restriction. No credentials or tokens are inspected.
///
/// # Errors
///
/// This static metadata function cannot fail.
pub fn byte_tos_auth_metadata() -> serde_json::Value {
    serde_json::json!({
        "modes": ["aksk", "zti"],
        "default": "aksk",
        "precedence": ["command_line", "profile.tos.auth_mode", "BYTETOS_AUTH_MODE", "aksk"],
        "zti_built_in": true,
        "zti_sources": zti_sources_for_platform(cfg!(unix)),
        "zti_fallback_to_aksk": false,
        "zti_presign_supported": false,
    })
}

fn zti_sources_for_platform(supports_agent: bool) -> Vec<&'static str> {
    // [Review Fix #4] Windows has no Unix Agent transport; metadata must not advertise it.
    let mut sources = vec!["SEC_TOKEN_STRING"];
    if supports_agent {
        sources.push("local_agent");
    }
    sources.push("SEC_TOKEN_PATH");
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_schema_documents_byte_modes_and_priority() {
        let parameter = byte_tos_auth_parameter();
        assert_eq!(parameter.name, "auth_mode");
        assert!(!parameter.required);
        assert!(matches!(
            parameter.location,
            crate::agent::describe::ParameterLocation::Flag
        ));
        assert_eq!(
            parameter.schema.unwrap(),
            serde_json::json!({
                "type": "string", "enum": ["aksk", "zti"], "default": "aksk"
            })
        );
        assert!(parameter
            .description
            .contains("--auth-mode > [profile.tos].auth_mode > BYTETOS_AUTH_MODE > aksk"));
        assert!(parameter.description.contains("no fallback"));
        assert_eq!(parameter.description.contains("a local Agent"), cfg!(unix));
    }

    #[test]
    fn token_sources_match_platform_capabilities() {
        assert_eq!(
            zti_sources_for_platform(true),
            vec!["SEC_TOKEN_STRING", "local_agent", "SEC_TOKEN_PATH"]
        );
        assert_eq!(
            zti_sources_for_platform(false),
            vec!["SEC_TOKEN_STRING", "SEC_TOKEN_PATH"]
        );
    }

    #[test]
    fn modes_normalize_and_have_stable_serialization() {
        for (input, expected) in [
            (" ZTI ", ByteTosAuthMode::Zti),
            ("AkSk", ByteTosAuthMode::Aksk),
        ] {
            assert_eq!(ByteTosAuthMode::parse(input, "test").unwrap(), expected);
            assert_eq!(serde_json::to_value(expected).unwrap(), expected.as_str());
        }
        for invalid in ["", "unified", "oauth", "zti-token"] {
            let error = ByteTosAuthMode::parse(invalid, "test source").unwrap_err();
            assert!(error.to_string().contains("test source"));
        }
    }
}
