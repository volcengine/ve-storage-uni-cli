use super::*;
use chrono::{TimeZone, Utc};

pub(super) fn token_json() -> String {
    r#"{"code":0,"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600,"refresh_token_expires_in":7200,"token_type":"Bearer","scope":"read:user offline_access"}"#.into()
}
fn now() -> chrono::NaiveDateTime {
    Utc.with_ymd_and_hms(2026, 11, 16, 0, 0, 0)
        .unwrap()
        .naive_utc()
}
fn parse(value: &str) -> Result<wire::TokenResponse, crate::login::ProviderError> {
    wire::parse_token(value.as_bytes(), &adapter().required_scopes, now())
}

#[test]
fn top_level_tokens_accept_additive_fields_and_use_actual_lifetimes() {
    let value = token_json().replace("\"code\":0", "\"code\":0,\"future\":{\"arbitrary\":[1,2]}");
    let tokens = parse(&value).unwrap();
    assert_eq!(tokens.expires_at, now() + chrono::Duration::seconds(3600));
    assert_eq!(
        tokens.refresh_expires_at,
        now() + chrono::Duration::seconds(7200)
    );
    assert_eq!(tokens.scope, vec!["offline_access", "read:user"]);
    assert_eq!(tokens.access_token.as_str(), "new-access");
}

#[test]
fn token_contract_rejects_missing_wrong_and_contradictory_fields() {
    let valid = token_json();
    for value in [
        format!(r#"{{"code":0,"data":{valid}}}"#),
        valid.replace("\"refresh_token\":\"new-refresh\",", ""),
        valid.replace("\"token_type\":\"Bearer\"", "\"token_type\":\"bearer\""),
        valid.replace("\"code\":0", "\"code\":0,\"error\":\"invalid_grant\""),
        valid.replace("\"code\":0", "\"code\":0,\"error_description\":null"),
        valid.replace("\"code\":0", "\"code\":0,\"code\":0"),
        valid.replace(
            "\"refresh_token\":",
            "\"refresh_token\":null,\"refresh_token\":",
        ),
        valid.replace("new-access", ""),
        valid.replace("new-access", "bad\\ntoken"),
    ] {
        assert!(parse(&value).is_err(), "invalid token response accepted");
    }
}

#[test]
fn duration_scope_and_token_bounds_fail_closed() {
    let valid = token_json();
    for value in [
        valid.replace("3600", "0"),
        valid.replace("3600", "-1"),
        valid.replace("7200", "9223372036854775807"),
        valid.replace(r#""refresh_token_expires_in":7200,"#, ""),
        valid.replace("7200", "0"),
        valid.replace("3600", "1.5"),
        valid.replace("read:user offline_access", "offline_access"),
        valid.replace(
            "read:user offline_access",
            "read:user offline_access read:user",
        ),
        valid.replace("read:user offline_access", "read:user  offline_access"),
        valid.replace("read:user offline_access", "read:user\\toffline_access"),
        valid.replace("new-access", &"x".repeat(16385)),
    ] {
        assert!(parse(&value).is_err());
    }
}

#[test]
fn userinfo_requires_positive_identity_and_never_infers_personal_accounts() {
    let value = r#"{"code":0,"data":{"open_id":"u","tenant_key":"t","future":null}}"#;
    let identity = wire::parse_identity(value.as_bytes()).unwrap();
    assert_eq!(identity.user_id, "u");
    assert_eq!(identity.tenant_id.as_deref(), Some("t"));
    for value in [
        r#"{"code":1,"data":{"open_id":"u","tenant_key":"t"}}"#,
        r#"{"code":0,"data":{"open_id":"u"}}"#,
        r#"{"code":0,"data":{"open_id":"u","tenant_key":null}}"#,
        r#"{"code":0,"data":{"open_id":"","tenant_key":"t"}}"#,
        r#"{"code":0,"data":{"open_id":"u","tenant_key":""}}"#,
        r#"{"code":0,"data":{"open_id":"u","open_id":"v","tenant_key":"t"}}"#,
        r#"{"code":0,"error":"denied","data":{"open_id":"u","tenant_key":"t"}}"#,
    ] {
        assert!(wire::parse_identity(value.as_bytes()).is_err());
    }
}

#[test]
fn documented_message_rejects_duplicates_null_and_wrong_types() {
    for message in [
        r#""msg":"success","msg":"again""#,
        r#""msg":null"#,
        r#""msg":42"#,
        r#""msg":{"nested":"text"}"#,
    ] {
        let response =
            format!(r#"{{"code":0,{message},"data":{{"open_id":"u","tenant_key":"t"}}}}"#);
        assert!(matches!(
            wire::parse_identity(response.as_bytes()),
            Err(ProviderError::Contract)
        ));
    }
}

#[test]
fn token_response_rejects_positional_arrays() {
    let response =
        r#"[0,"new-access","new-refresh",3600,7200,"Bearer","read:user offline_access"]"#;
    assert!(matches!(parse(response), Err(ProviderError::Contract)));
}

#[test]
fn userinfo_response_rejects_root_positional_arrays() {
    let response = br#"[0,"success",{"open_id":"u","tenant_key":"t"}]"#;
    assert!(matches!(
        wire::parse_identity(response),
        Err(ProviderError::Contract)
    ));
}

#[test]
fn userinfo_response_rejects_nested_identity_positional_arrays() {
    let response = br#"{"code":0,"data":["u","t"]}"#;
    assert!(matches!(
        wire::parse_identity(response),
        Err(ProviderError::Contract)
    ));
}

fn nested_arrays(depth: usize) -> String {
    format!("{}0{}", "[".repeat(depth), "]".repeat(depth))
}

#[test]
fn token_response_rejects_deep_ignored_subtrees_within_body_bound() {
    let response = token_json().replace(
        "\"code\":0",
        &format!("\"code\":0,\"future\":{}", nested_arrays(10_000)),
    );
    assert!(response.len() <= TOKEN_BODY_LIMIT);
    assert!(matches!(parse(&response), Err(ProviderError::Contract)));
}

#[test]
fn userinfo_response_rejects_deep_ignored_subtrees_at_each_object_level() {
    for response in [
        format!(
            r#"{{"code":0,"future":{},"data":{{"open_id":"u","tenant_key":"t"}}}}"#,
            nested_arrays(10_000)
        ),
        format!(
            r#"{{"code":0,"data":{{"open_id":"u","tenant_key":"t","future":{}}}}}"#,
            nested_arrays(10_000)
        ),
    ] {
        assert!(response.len() <= USERINFO_BODY_LIMIT);
        assert!(matches!(
            wire::parse_identity(response.as_bytes()),
            Err(ProviderError::Contract)
        ));
    }
}

#[test]
fn quoted_brackets_and_escaped_quotes_remain_compatible() {
    let quoted = serde_json::to_string(&format!(
        r#"{}\"{}"#,
        "[".repeat(10_000),
        "]".repeat(10_000)
    ))
    .unwrap();
    let response = token_json().replace("\"code\":0", &format!("\"code\":0,\"future\":{quoted}"));
    assert!(parse(&response).is_ok());
    let response =
        format!(r#"{{"code":0,"data":{{"open_id":"u","tenant_key":"t","future":{quoted}}}}}"#);
    assert!(wire::parse_identity(response.as_bytes()).is_ok());
}

#[test]
fn oauth_depth_boundary_counts_each_schema_container() {
    for depth in [63, 64] {
        let response = token_json().replace(
            "\"code\":0",
            &format!("\"code\":0,\"future\":{}", nested_arrays(depth)),
        );
        assert_eq!(parse(&response).is_ok(), depth == 63);
        let response = format!(
            r#"{{"code":0,"future":{},"data":{{"open_id":"u","tenant_key":"t"}}}}"#,
            nested_arrays(depth)
        );
        assert_eq!(
            wire::parse_identity(response.as_bytes()).is_ok(),
            depth == 63
        );
    }
    for depth in [62, 63] {
        let response = format!(
            r#"{{"code":0,"data":{{"open_id":"u","tenant_key":"t","future":{}}}}}"#,
            nested_arrays(depth)
        );
        assert_eq!(
            wire::parse_identity(response.as_bytes()).is_ok(),
            depth == 62
        );
    }
}
