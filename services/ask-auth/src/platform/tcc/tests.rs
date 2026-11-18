use super::credentials::validate_token;
use super::*;
use crate::platform::config::ExpectedEnvironment;
use std::{collections::VecDeque, sync::Mutex};
fn locator() -> Locator {
    Locator {
        namespace: "security.ask_auth".into(),
        space: "default".into(),
        environment: ExpectedEnvironment::Boe,
    }
}
fn document(key: &str, value: String) -> RawDocument {
    RawDocument {
        key: key.into(),
        value: SensitiveText::new(value),
    }
}
fn meta(value: &str) -> RawDocument {
    document("/tcc/v2/meta/security.ask_auth", value.into())
}
fn content(value: &str) -> RawDocument {
    document("/tcc/v2/data/security.ask_auth/default", value.into())
}
const GOOD_META: &str = r#"{"version_codes":[{"confspace":"default","version_code":"42"}]}"#;
const GOOD_CONTENT: &str = r#"{"version_code":"42","data":{"ask_auth_runtime":"runtime-secret","ask_auth_secrets":"app-secret","ask_auth_admission":"admission-secret","ask_auth_cleanup":"cleanup-secret","unknown":"discard-secret"}}"#;
fn reject(meta_value: &str, content_value: &str) {
    assert!(matches!(
        decode_snapshot(meta(meta_value), content(content_value), &locator()),
        Err(TccError::InvalidSnapshot)
    ));
}
#[test]
fn valid_locator_is_accepted_without_platform_initialization() {
    assert!(Locator::new("security.ask_auth", "default", ExpectedEnvironment::Boe).is_ok());
}
#[test]
fn unsafe_locator_syntax_is_rejected() {
    for namespace in ["", ".a", "a.", "a..b", "../a", "a/b", "a?b", "a\nb", "中文"] {
        assert!(Locator::new(namespace, "default", ExpectedEnvironment::Boe).is_err());
    }
    for space in ["", "a.b", "a/b", "a?b", "a\rb", "中文"] {
        assert!(Locator::new("safe.psm", space, ExpectedEnvironment::Boe).is_err());
    }
    assert!(Locator::new(&"a".repeat(257), "default", ExpectedEnvironment::Boe).is_err());
    assert!(Locator::new("safe.psm", &"a".repeat(65), ExpectedEnvironment::Boe).is_err());
}
#[test]
fn snapshot_selects_only_typed_keys_and_marks_read_completion() {
    let before = Instant::now();
    let snapshot = decode_snapshot(meta(GOOD_META), content(GOOD_CONTENT), &locator())
        .ok()
        .unwrap();
    assert_eq!(snapshot.version(), "42");
    assert_eq!(snapshot.required(PayloadKey::Runtime), Ok("runtime-secret"));
    assert_eq!(snapshot.required(PayloadKey::Secrets), Ok("app-secret"));
    assert_eq!(
        snapshot.required(PayloadKey::Admission),
        Ok("admission-secret")
    );
    assert_eq!(snapshot.required(PayloadKey::Cleanup), Ok("cleanup-secret"));
    assert!(
        snapshot.read_completed_at() >= before && snapshot.read_completed_at() <= Instant::now()
    );
}
#[test]
fn missing_payload_remains_explicitly_missing() {
    let snapshot = decode_snapshot(
        meta(GOOD_META),
        content(r#"{"version_code":"42","data":{}}"#),
        &locator(),
    )
    .ok()
    .unwrap();
    assert_eq!(
        snapshot.required(PayloadKey::Runtime),
        Err(TccError::MissingConfiguration)
    );
}
#[test]
fn confspace_must_exist_exactly_once() {
    for value in [
        r#"{"version_codes":[]}"#,
        r#"{"version_codes":[{"confspace":"other","version_code":"42"}]}"#,
        r#"{"version_codes":[{"confspace":"default","version_code":"42"},{"confspace":"default","version_code":"42"}]}"#,
    ] {
        reject(value, GOOD_CONTENT);
    }
}
#[test]
fn matching_bounded_nonempty_versions_are_required() {
    for version in ["", "different", "bad\nversion", &"a".repeat(257)] {
        reject(
            GOOD_META,
            &serde_json::json!({"version_code":version,"data":{}}).to_string(),
        );
    }
    reject(
        r#"{"version_codes":[{"confspace":"default","version_code":""}]}"#,
        r#"{"version_code":"","data":{}}"#,
    );
}
#[test]
fn response_resources_must_match_without_suffix_acceptance() {
    for wrong in [
        "/other/tcc/v2/meta/security.ask_auth",
        "//tcc/v2/meta/security.ask_auth",
        "/tcc/v2/meta/other",
    ] {
        let mut response = meta(GOOD_META);
        response.key = wrong.into();
        assert!(decode_snapshot(response, content(GOOD_CONTENT), &locator()).is_err());
    }
    let mut response = content(GOOD_CONTENT);
    response.key.push_str("/other");
    assert!(decode_snapshot(meta(GOOD_META), response, &locator()).is_err());
    let mut response = meta(GOOD_META);
    response.key.remove(0);
    assert!(decode_snapshot(response, content(GOOD_CONTENT), &locator()).is_ok());
}
#[test]
fn all_nonempty_gray_overrides_are_rejected() {
    for gray in [
        r#"{"data":{"unknown":"secret"}}"#,
        r#"{"gray_code":"code"}"#,
        r#"{"gray_ip_list":["127.0.0.1"]}"#,
    ] {
        reject(
            GOOD_META,
            &format!(r#"{{"version_code":"42","data":{{}},"gray_data":{gray}}}"#),
        );
    }
}
#[test]
fn absent_null_and_empty_gray_are_accepted() {
    for gray in [
        "null",
        "{}",
        r#"{"data":{},"gray_code":"","gray_ip_list":[]}"#,
    ] {
        let value = format!(r#"{{"version_code":"42","data":{{}},"gray_data":{gray}}}"#);
        assert!(decode_snapshot(meta(GOOD_META), content(&value), &locator()).is_ok());
    }
}
#[test]
fn invalid_json_and_schema_types_are_rejected() {
    for value in [
        "not json",
        "[]",
        "null",
        r#"{"version_code":42,"data":{}}"#,
        r#"{"version_code":"42","data":{"unknown":42}}"#,
        r#"{"version_code":"42","data":[],"gray_data":null}"#,
        r#"{"version_code":"42","data":{},"gray_data":{"gray_ip_list":"bad"}}"#,
    ] {
        reject(GOOD_META, value);
    }
    reject(r#"{"version_codes":"wrong"}"#, GOOD_CONTENT);
}
#[test]
fn duplicate_recognized_fields_and_mapping_keys_are_rejected() {
    for value in [
        r#"{"version_codes":[],"version_codes":[]}"#,
        r#"{"version_codes":[{"confspace":"default","confspace":"default","version_code":"42"}]}"#,
        r#"{"version_codes":[{"confspace":"default","version_code":"42","version_code":"42"}]}"#,
    ] {
        reject(value, GOOD_CONTENT);
    }
    for value in [
        r#"{"version_code":"42","version_code":"42","data":{}}"#,
        r#"{"version_code":"42","data":{},"data":{}}"#,
        r#"{"version_code":"42","data":{},"gray_data":null,"gray_data":null}"#,
        r#"{"version_code":"42","data":{"unknown":"a","unknown":"b"}}"#,
    ] {
        reject(GOOD_META, value);
    }
    for field in ["data", "gray_code", "gray_ip_list"] {
        let empty = match field {
            "data" => "{}",
            "gray_code" => "null",
            _ => "null",
        };
        reject(
            GOOD_META,
            &format!(
                r#"{{"version_code":"42","data":{{}},"gray_data":{{"{field}":{empty},"{field}":{empty}}}}}"#
            ),
        );
    }
}
#[test]
fn raw_document_and_collection_bounds_are_enforced() {
    let mut response = meta(GOOD_META);
    response.value = SensitiveText::new(format!("{}{}", " ".repeat(MAX_META_BYTES + 1), GOOD_META));
    assert!(decode_snapshot(response, content(GOOD_CONTENT), &locator()).is_err());
    let mut response = content(GOOD_CONTENT);
    response.value = SensitiveText::new(format!(
        "{}{}",
        " ".repeat(MAX_CONTENT_BYTES + 1),
        GOOD_CONTENT
    ));
    assert!(decode_snapshot(meta(GOOD_META), response, &locator()).is_err());
    let mut spaces =
        vec![serde_json::json!({"confspace":"other","version_code":"42"}); MAX_ENTRIES + 1];
    spaces[0] = serde_json::json!({"confspace":"default","version_code":"42"});
    reject(
        &serde_json::json!({"version_codes":spaces}).to_string(),
        GOOD_CONTENT,
    );
    let entries: serde_json::Map<String, serde_json::Value> = (0..=MAX_ENTRIES)
        .map(|index| (format!("key{index}"), serde_json::json!("value")))
        .collect();
    reject(
        GOOD_META,
        &serde_json::json!({"version_code":"42","data":entries}).to_string(),
    );
    reject(GOOD_META,&serde_json::json!({"version_code":"42","data":{"ask_auth_secrets":"s".repeat(MAX_PAYLOAD_BYTES+1)}}).to_string());
}
struct FakeReader {
    responses: Mutex<VecDeque<Result<RawDocument, TccError>>>,
    requested: Mutex<Vec<String>>,
    is_pending: bool,
    delay: Duration,
}
impl RawReader for FakeReader {
    fn read<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<RawDocument, TccError>> + Send + 'a>> {
        Box::pin(async move {
            self.requested.lock().unwrap().push(key.into());
            if self.is_pending {
                return std::future::pending().await;
            }
            tokio::time::sleep(self.delay).await;
            self.responses.lock().unwrap().pop_front().unwrap()
        })
    }
}
fn fake(responses: Vec<Result<RawDocument, TccError>>) -> FakeReader {
    FakeReader {
        responses: Mutex::new(responses.into()),
        requested: Mutex::new(Vec::new()),
        is_pending: false,
        delay: Duration::ZERO,
    }
}
#[tokio::test]
async fn each_snapshot_reads_again_and_never_reuses_previous_value() {
    let reader = fake(vec![
        Ok(meta(GOOD_META)),
        Ok(content(GOOD_CONTENT)),
        Err(TccError::Unavailable),
    ]);
    assert!(read_snapshot(&reader, &locator()).await.is_ok());
    assert!(matches!(
        read_snapshot(&reader, &locator()).await,
        Err(TccError::Unavailable)
    ));
    assert_eq!(
        *reader.requested.lock().unwrap(),
        vec![
            "/tcc/v2/meta/security.ask_auth",
            "/tcc/v2/data/security.ask_auth/default",
            "/tcc/v2/meta/security.ask_auth"
        ]
    );
}
#[tokio::test(start_paused = true)]
async fn total_snapshot_timeout_is_bounded() {
    let reader = FakeReader {
        is_pending: true,
        ..fake(vec![])
    };
    assert!(matches!(
        read_snapshot(&reader, &locator()).await,
        Err(TccError::Timeout)
    ));
}
#[test]
fn core_dependency_compiles_with_adapter() {
    let _router = crate::build_router();
}

#[test]
fn errors_expose_fixed_categories_without_upstream_messages() {
    let categories = [
        TccError::InvalidLocator,
        TccError::Environment,
        TccError::Identity,
        TccError::Unavailable,
        TccError::Timeout,
        TccError::InvalidSnapshot,
        TccError::MissingConfiguration,
    ];
    for error in categories {
        assert!(!format!("{error} {error:?}").contains("credential-sentinel"));
        assert!(std::error::Error::source(&error).is_none());
    }
}

#[test]
fn positional_arrays_cannot_masquerade_as_schema_objects() {
    reject(
        r#"[[{"confspace":"default","version_code":"42"}]]"#,
        GOOD_CONTENT,
    );
    reject(r#"{"version_codes":[["default","42"]]}"#, GOOD_CONTENT);
    reject(GOOD_META, r#"["42",{},null]"#);
    reject(
        GOOD_META,
        r#"{"version_code":"42","data":{},"gray_data":[{},null,null]}"#,
    );
}
#[test]
fn constructor_requires_tokio_before_client_construction() {
    assert!(matches!(
        TccSource::new(locator()),
        Err(TccError::Unavailable)
    ));
}

#[tokio::test(start_paused = true)]
async fn two_reads_share_one_total_deadline() {
    let reader = FakeReader {
        delay: Duration::from_secs(2),
        ..fake(vec![Ok(meta(GOOD_META)), Ok(content(GOOD_CONTENT))])
    };
    let started = tokio::time::Instant::now();
    assert!(matches!(
        read_snapshot(&reader, &locator()).await,
        Err(TccError::Timeout)
    ));
    assert_eq!(started.elapsed(), SNAPSHOT_TIMEOUT);
    assert_eq!(reader.requested.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn repeated_versions_still_fetch_new_payloads_and_content_failure_is_fatal() {
    let changed = GOOD_CONTENT.replace("runtime-secret", "new-runtime-secret");
    let reader = fake(vec![
        Ok(meta(GOOD_META)),
        Ok(content(GOOD_CONTENT)),
        Ok(meta(GOOD_META)),
        Ok(content(&changed)),
        Ok(meta(GOOD_META)),
        Err(TccError::Unavailable),
    ]);
    let first = read_snapshot(&reader, &locator()).await.ok().unwrap();
    let second = read_snapshot(&reader, &locator()).await.ok().unwrap();
    assert_eq!(first.required(PayloadKey::Runtime), Ok("runtime-secret"));
    assert_eq!(
        second.required(PayloadKey::Runtime),
        Ok("new-runtime-secret")
    );
    assert!(matches!(
        read_snapshot(&reader, &locator()).await,
        Err(TccError::Unavailable)
    ));
    assert_eq!(reader.requested.lock().unwrap().len(), 6);
}
#[test]
fn exact_raw_document_limits_accept_valid_documents() {
    let meta_value = format!(
        "{}{}",
        " ".repeat(MAX_META_BYTES - GOOD_META.len()),
        GOOD_META
    );
    let content_value = format!(
        "{}{}",
        " ".repeat(MAX_CONTENT_BYTES - GOOD_CONTENT.len()),
        GOOD_CONTENT
    );
    assert!(decode_snapshot(meta(&meta_value), content(&content_value), &locator()).is_ok());
}
#[test]
fn exact_collection_and_payload_limits_are_accepted() {
    let mut spaces =
        vec![serde_json::json!({"confspace":"other","version_code":"42"}); MAX_ENTRIES];
    spaces[0] = serde_json::json!({"confspace":"default","version_code":"42"});
    let metadata = serde_json::json!({"version_codes":spaces}).to_string();
    let mut entries: serde_json::Map<String, serde_json::Value> = (0..MAX_ENTRIES - 4)
        .map(|index| (format!("key{index}"), serde_json::json!("value")))
        .collect();
    for key in [
        PayloadKey::Runtime,
        PayloadKey::Secrets,
        PayloadKey::Admission,
        PayloadKey::Cleanup,
    ] {
        entries.insert(
            key.name().into(),
            serde_json::json!("s".repeat(MAX_PAYLOAD_BYTES)),
        );
    }
    let payloads = serde_json::json!({"version_code":"42","data":entries}).to_string();
    assert!(decode_snapshot(meta(&metadata), content(&payloads), &locator()).is_ok());
}
#[test]
fn invalid_input_never_appears_in_returned_errors() {
    let invalid = r#"{"version_code":"42","data":{"ask_auth_secrets":credential-sentinel}}"#;
    match decode_snapshot(meta(GOOD_META), content(invalid), &locator()) {
        Err(error) => assert_eq!(format!("{error}"), "invalid TCC snapshot"),
        Ok(_) => panic!("invalid document accepted"),
    }
}

#[test]
fn deep_additive_unknown_fields_are_rejected_before_typed_parsing() {
    let unknown = format!("{}0{}", "[".repeat(80), "]".repeat(80));
    let metadata = format!(
        r#"{{"version_codes":[{{"confspace":"default","version_code":"42"}}],"unknown":{unknown}}}"#
    );
    let payloads = format!(r#"{{"version_code":"42","data":{{}},"unknown":{unknown}}}"#);
    reject(&metadata, GOOD_CONTENT);
    reject(GOOD_META, &payloads);
}

struct BlockingReader;
impl RawReader for BlockingReader {
    fn read<'a>(&'a self, key: &'a str) -> ReadFuture<'a> {
        Box::pin(async move {
            if key.contains("/meta/") {
                return Ok(meta(GOOD_META));
            }
            std::thread::sleep(SNAPSHOT_TIMEOUT + Duration::from_millis(25));
            Ok(content(GOOD_CONTENT))
        })
    }
}
#[tokio::test]
async fn synchronous_completion_after_deadline_is_rejected() {
    assert!(matches!(
        read_snapshot(&BlockingReader, &locator()).await,
        Err(TccError::Timeout)
    ));
}

#[test]
fn ten_thousand_level_unknown_subtrees_are_rejected_in_both_documents() {
    let unknown = format!("{}0{}", "[".repeat(10_000), "]".repeat(10_000));
    let metadata = format!(
        r#"{{"version_codes":[{{"confspace":"default","version_code":"42"}}],"unknown":{unknown}}}"#
    );
    let payloads = format!(r#"{{"version_code":"42","data":{{}},"unknown":{unknown}}}"#);
    reject(&metadata, GOOD_CONTENT);
    reject(GOOD_META, &payloads);
}

#[test]
fn shallow_unknown_additive_fields_remain_compatible_at_depth_boundary() {
    let unknown = format!("{}0{}", "[".repeat(63), "]".repeat(63));
    let metadata = format!(
        r#"{{"version_codes":[{{"confspace":"default","version_code":"42"}}],"unknown":{unknown}}}"#
    );
    let payloads = format!(r#"{{"version_code":"42","data":{{}},"unknown":{unknown}}}"#);
    assert!(decode_snapshot(meta(&metadata), content(&payloads), &locator()).is_ok());
}

#[test]
fn dps_credentials_must_be_bounded_visible_http_header_values() {
    for token in [
        " ",
        "secret\n",
        "secret\r",
        "secret\t",
        "secret secret",
        "密钥",
    ] {
        assert_eq!(validate_token(Some(token)), Err(TccError::Identity));
    }
    assert_eq!(
        validate_token(Some(&"x".repeat(16 * 1024 + 1))),
        Err(TccError::Identity)
    );
    assert_eq!(validate_token(Some("visible.token-value_123")), Ok(()));
}
