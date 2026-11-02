//! Types and validation for ADrive V1 share links and anonymous access.

use super::client::{Error, HasResponseInfo, Result};
use super::types::ResponseInfo;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

/// Create one file/folder share or an entire Space; optional settings retain explicit zero/false values.
#[derive(Clone, Default, Serialize)]
pub struct CreateShareLinkInput {
    #[serde(rename = "InstanceID")]
    pub instance_id: String,
    #[serde(rename = "SpaceID")]
    pub space_id: String,
    #[serde(rename = "FilePathList")]
    pub file_path_list: Vec<String>,
    /// Select the entire Space instead of one path; the path list must then be empty.
    #[serde(rename = "ShareAllFiles", skip_serializing_if = "is_false")]
    pub is_share_all_files: bool,
    #[serde(rename = "SharePwd", skip_serializing_if = "Option::is_none")]
    pub share_pwd: Option<String>,
    #[serde(rename = "ExpirationSec", skip_serializing_if = "Option::is_none")]
    pub expiration_sec: Option<i64>,
    #[serde(rename = "Description", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "ShareName", skip_serializing_if = "Option::is_none")]
    pub share_name: Option<String>,
    #[serde(rename = "Permission", skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    #[serde(rename = "RequireLogin", skip_serializing_if = "Option::is_none")]
    pub require_login: Option<bool>,
    #[serde(rename = "PreviewLimit", skip_serializing_if = "Option::is_none")]
    pub preview_limit: Option<i64>,
    #[serde(rename = "DownloadLimit", skip_serializing_if = "Option::is_none")]
    pub download_limit: Option<i64>,
    #[serde(rename = "SaveLimit", skip_serializing_if = "Option::is_none")]
    pub save_limit: Option<i64>,
    #[serde(rename = "Extra", skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
}

impl CreateShareLinkInput {
    /// Validate identifiers, the exclusive whole-Space/single-path scope, permission and limits.
    /// Returns an input error before any request if the V1 product contract is violated.
    pub fn validate(&self) -> Result<()> {
        validate_required(&self.instance_id, "InstanceID")?;
        validate_required(&self.space_id, "SpaceID")?;
        if self.is_share_all_files && !self.file_path_list.is_empty() {
            return Err(Error::Client(
                "ShareAllFiles must not be combined with FilePathList entries".into(),
            ));
        }
        if !self.is_share_all_files && self.file_path_list.len() != 1 {
            return Err(Error::Client(
                "FilePathList must contain exactly one file or folder".into(),
            ));
        }
        if !self.is_share_all_files {
            validate_required(&self.file_path_list[0], "FilePathList")?;
        }
        validate_permission(self.permission.as_deref())?;
        validate_limits([
            self.expiration_sec,
            self.preview_limit,
            self.download_limit,
            self.save_limit,
        ])
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Update only explicitly selected mutable fields, with optimistic concurrency timestamp.
#[derive(Clone, Default, Serialize)]
pub struct UpdateShareLinkInput {
    #[serde(skip)]
    pub instance_id: String,
    #[serde(rename = "ShareID")]
    pub share_id: String,
    #[serde(rename = "UpdatedAt")]
    pub updated_at: i64,
    #[serde(rename = "SharePwd", skip_serializing_if = "Option::is_none")]
    pub share_pwd: Option<String>,
    #[serde(rename = "ExpirationSec", skip_serializing_if = "Option::is_none")]
    pub expiration_sec: Option<i64>,
    #[serde(rename = "Description", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "ShareName", skip_serializing_if = "Option::is_none")]
    pub share_name: Option<String>,
    #[serde(rename = "Permission", skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    #[serde(rename = "RequireLogin", skip_serializing_if = "Option::is_none")]
    pub require_login: Option<bool>,
    #[serde(rename = "PreviewLimit", skip_serializing_if = "Option::is_none")]
    pub preview_limit: Option<i64>,
    #[serde(rename = "DownloadLimit", skip_serializing_if = "Option::is_none")]
    pub download_limit: Option<i64>,
    #[serde(rename = "SaveLimit", skip_serializing_if = "Option::is_none")]
    pub save_limit: Option<i64>,
    #[serde(rename = "Extra", skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
}

impl UpdateShareLinkInput {
    /// Validate identifiers, timestamp and explicitly supplied permission/limit values.
    /// Returns an input error before submitting an invalid mutation.
    pub fn validate(&self) -> Result<()> {
        validate_required(&self.instance_id, "InstanceID")?;
        validate_required(&self.share_id, "ShareID")?;
        validate_permission(self.permission.as_deref())?;
        validate_limits([
            self.expiration_sec,
            self.preview_limit,
            self.download_limit,
            self.save_limit,
        ])?;
        validate_limits([Some(self.updated_at)])
    }
}

/// Local Instance binding and service ShareID for a management read.
#[derive(Clone, Debug, Default)]
pub struct GetShareLinkInput {
    pub instance_id: String,
    pub share_id: String,
}

/// Local Instance binding and service ShareID for cancellation.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CancelShareLinkInput {
    #[serde(skip)]
    pub instance_id: String,
    #[serde(rename = "ShareID")]
    pub share_id: String,
}

/// Share search filters. Creators are mandatory even for administrators.
#[derive(Clone, Debug, Default)]
pub struct SearchShareLinksInput {
    pub instance_id: String,
    pub creators: Vec<String>,
    pub status: Option<String>,
    pub share_name: Option<String>,
    pub created_at_start: Option<String>,
    pub created_at_end: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

impl SearchShareLinksInput {
    /// Validate creators, status, RFC3339 times and pagination before requesting.
    /// Returns an input error for empty creators, negative values or offset > 10000.
    pub fn validate(&self) -> Result<()> {
        validate_required(&self.instance_id, "InstanceID")?;
        if self.creators.is_empty() {
            return Err(Error::Client("at least one creator is required".into()));
        }
        for creator in &self.creators {
            validate_required(creator, "creator")?;
        }
        if self
            .status
            .as_deref()
            .is_some_and(|status| !matches!(status, "active" | "expired" | "revoked"))
        {
            return Err(Error::Client(
                "status must be active, expired or revoked".into(),
            ));
        }
        validate_search_times(
            self.created_at_start.as_deref(),
            self.created_at_end.as_deref(),
        )?;
        if self
            .offset
            .is_some_and(|offset| !(0..=10000).contains(&offset))
        {
            return Err(Error::Client("offset must be between 0 and 10000".into()));
        }
        validate_page_limit(self.limit)
    }

    /// Build SDK query pairs, retaining each creator as a repeated `creators` key.
    pub fn to_query_pairs(&self) -> Vec<(String, String)> {
        let mut query = vec![("instance_id".into(), self.instance_id.clone())];
        query.extend(
            self.creators
                .iter()
                .map(|creator| ("creators".into(), creator.clone())),
        );
        append_string(&mut query, "status", self.status.as_deref());
        append_string(&mut query, "share_name", self.share_name.as_deref());
        append_string(
            &mut query,
            "created_at_start",
            self.created_at_start.as_deref(),
        );
        append_string(&mut query, "created_at_end", self.created_at_end.as_deref());
        append_integer(&mut query, "offset", self.offset);
        append_integer(&mut query, "limit", self.limit);
        query
    }
}

/// Share metadata from management or anonymous access, preserving absent/unknown fields.
/// Anonymous callers can inspect `has_password`, `is_expired`, `is_cancelled`,
/// `remaining_download_count` and `require_login` without management credentials.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ShareLinkInfo {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(rename = "ShareID", skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    /// Service-provided page URL; may include a delivery password query.
    #[serde(rename = "ShareURL", skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
    #[serde(rename = "InstanceID", skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(rename = "SpaceID", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
    /// May be absent from anonymous metadata; absence must not be treated as false.
    #[serde(rename = "ShareAllFiles", skip_serializing_if = "Option::is_none")]
    pub is_share_all_files: Option<bool>,
    #[serde(rename = "Expiration", skip_serializing_if = "Option::is_none")]
    pub expiration: Option<String>,
    #[serde(rename = "Permission", skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    #[serde(rename = "RequireLogin", skip_serializing_if = "Option::is_none")]
    pub require_login: Option<bool>,
    #[serde(rename = "PreviewLimit", skip_serializing_if = "Option::is_none")]
    pub preview_limit: Option<i64>,
    #[serde(rename = "DownloadLimit", skip_serializing_if = "Option::is_none")]
    pub download_limit: Option<i64>,
    #[serde(rename = "SaveLimit", skip_serializing_if = "Option::is_none")]
    pub save_limit: Option<i64>,
    #[serde(rename = "PreviewCount", skip_serializing_if = "Option::is_none")]
    pub preview_count: Option<i64>,
    #[serde(rename = "DownloadCount", skip_serializing_if = "Option::is_none")]
    pub download_count: Option<i64>,
    #[serde(rename = "SaveCount", skip_serializing_if = "Option::is_none")]
    pub save_count: Option<i64>,
    #[serde(rename = "Description", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "ShareName", skip_serializing_if = "Option::is_none")]
    pub share_name: Option<String>,
    #[serde(rename = "Extra", skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
    #[serde(rename = "CreatorName", skip_serializing_if = "Option::is_none")]
    pub creator_name: Option<String>,
    #[serde(rename = "CreatorID", skip_serializing_if = "Option::is_none")]
    pub creator_id: Option<String>,
    #[serde(rename = "SharePwd", skip_serializing_if = "Option::is_none")]
    pub share_pwd: Option<String>,
    #[serde(rename = "Status", skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "CreatedAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(rename = "UpdatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(rename = "HasPassword", skip_serializing_if = "Option::is_none")]
    pub has_password: Option<bool>,
    #[serde(rename = "IsExpired", skip_serializing_if = "Option::is_none")]
    pub is_expired: Option<bool>,
    #[serde(rename = "IsCancelled", skip_serializing_if = "Option::is_none")]
    pub is_cancelled: Option<bool>,
    #[serde(
        rename = "RemainingDownloadCount",
        skip_serializing_if = "Option::is_none"
    )]
    pub remaining_download_count: Option<i64>,
    #[serde(
        rename = "FilePathList",
        default,
        deserialize_with = "deserialize_file_paths",
        skip_serializing_if = "Option::is_none"
    )]
    pub file_path_list: Option<Vec<String>>,
    #[serde(flatten)]
    pub additional_fields: BTreeMap<String, Value>,
}

impl fmt::Debug for ShareLinkInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShareLinkInfo")
            .field("has_status", &self.status.is_some())
            .field("has_password", &self.has_password)
            .finish_non_exhaustive()
    }
}

impl HasResponseInfo for ShareLinkInfo {
    fn set_response_info(&mut self, info: ResponseInfo) {
        self.response_info = info;
    }
}

/// Search results, including the service's total and page coordinates when supplied.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SearchShareLinksOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(rename = "Items", default)]
    pub items: Vec<ShareLinkInfo>,
    #[serde(rename = "Total", skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(rename = "Offset", skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(rename = "Limit", skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(flatten)]
    pub additional_fields: BTreeMap<String, Value>,
}

// [Review Fix #2] Unknown additions and response headers may contain credentials.
impl fmt::Debug for SearchShareLinksOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchShareLinksOutput")
            .field("item_count", &self.items.len())
            .field("total", &self.total)
            .field("offset", &self.offset)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl HasResponseInfo for SearchShareLinksOutput {
    fn set_response_info(&mut self, info: ResponseInfo) {
        self.response_info = info;
    }
}

/// Password exchange request. Passwords have no Debug representation.
#[derive(Clone, Default, Serialize)]
pub struct GetShareAccessTokenInput {
    #[serde(rename = "ShareID")]
    pub share_id: String,
    #[serde(rename = "SharePwd", skip_serializing_if = "Option::is_none")]
    pub share_pwd: Option<String>,
    #[serde(rename = "ExpireSec", skip_serializing_if = "Option::is_none")]
    pub expire_sec: Option<i64>,
}

/// Ephemeral token exchange result; deliberately cannot be serialized.
#[derive(Clone, Deserialize)]
pub struct ShareAccessTokenOutput {
    #[serde(rename = "AccessToken")]
    pub access_token: String,
    #[serde(rename = "ExpiresIn")]
    pub expires_in: i64,
}

impl fmt::Debug for ShareAccessTokenOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShareAccessTokenOutput")
            .field("access_token", &"***")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// Shared listing scope: exactly a nonempty prefix or `list_root: Some(true)`.
#[derive(Clone, Debug, Default)]
pub struct ShareListFilesInput {
    pub prefix: Option<String>,
    pub delimiter: Option<String>,
    pub limit: Option<i64>,
    pub marker: Option<String>,
    pub list_root: Option<bool>,
}

impl ShareListFilesInput {
    /// Validate the exclusive listing scope and positive page limit.
    /// Returns a local input error when no scope or conflicting scopes are selected.
    pub fn validate(&self) -> Result<()> {
        let has_prefix = self
            .prefix
            .as_deref()
            .is_some_and(|prefix| !prefix.is_empty());
        if has_prefix == (self.list_root == Some(true)) || self.prefix.as_deref() == Some("") {
            return Err(Error::Client(
                "exactly one nonempty Prefix or ListRoot=true is required".into(),
            ));
        }
        if let Some(prefix) = &self.prefix {
            validate_required(prefix, "Prefix")?;
        }
        validate_page_limit(self.limit)
    }

    /// Build snake_case service query pairs without inventing an empty prefix.
    pub fn to_query_pairs(&self) -> Vec<(String, String)> {
        let mut query = Vec::new();
        append_string(&mut query, "prefix", self.prefix.as_deref());
        append_string(&mut query, "delimiter", self.delimiter.as_deref());
        append_string(&mut query, "marker", self.marker.as_deref());
        append_integer(&mut query, "limit", self.limit);
        if self.list_root == Some(true) {
            query.push(("list_root".into(), "true".into()));
        }
        query
    }
}

/// Service-issued download URL. Its Debug representation hides URL credentials.
#[derive(Clone, Deserialize)]
pub struct ShareDownloadUrlOutput {
    #[serde(rename = "DownloadURL")]
    pub download_url: String,
}

impl fmt::Debug for ShareDownloadUrlOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShareDownloadUrlOutput")
            .finish_non_exhaustive()
    }
}

pub(crate) fn validate_required(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(Error::Client(format!(
            "{name} must be nonempty and contain no control characters"
        )));
    }
    Ok(())
}

fn validate_permission(permission: Option<&str>) -> Result<()> {
    if permission.is_some_and(|value| !matches!(value, "view" | "download")) {
        return Err(Error::Client("Permission must be view or download".into()));
    }
    Ok(())
}

fn validate_limits<const N: usize>(limits: [Option<i64>; N]) -> Result<()> {
    if limits.into_iter().flatten().any(|value| value < 0) {
        return Err(Error::Client(
            "share limits and timestamps must be nonnegative".into(),
        ));
    }
    Ok(())
}

fn validate_page_limit(limit: Option<i64>) -> Result<()> {
    if limit.is_some_and(|value| value <= 0) {
        return Err(Error::Client("limit must be positive".into()));
    }
    Ok(())
}

fn validate_search_times(start: Option<&str>, end: Option<&str>) -> Result<()> {
    let parse = |value: &str| {
        chrono::DateTime::parse_from_rfc3339(value)
            .map_err(|_| Error::Client("search times must use RFC3339".into()))
    };
    let start = start.map(parse).transpose()?;
    let end = end.map(parse).transpose()?;
    if matches!((start, end), (Some(start), Some(end)) if start > end) {
        return Err(Error::Client(
            "CreatedAtStart must not be later than CreatedAtEnd".into(),
        ));
    }
    Ok(())
}

fn append_string(query: &mut Vec<(String, String)>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        query.push((key.into(), value.into()));
    }
}

fn append_integer(query: &mut Vec<(String, String)>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

fn deserialize_file_paths<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(Value::String(encoded)) => serde_json::from_str(&encoded)
            .map(Some)
            .map_err(serde::de::Error::custom),
        Some(value) => serde_json::from_value(value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_scope_selects_exactly_whole_space_or_one_nonempty_path() {
        let mut input = CreateShareLinkInput {
            instance_id: "inst".into(),
            space_id: "space".into(),
            is_share_all_files: true,
            ..Default::default()
        };
        input.validate().unwrap();
        let encoded = serde_json::to_value(&input).unwrap();
        assert_eq!(encoded["ShareAllFiles"], true);
        assert_eq!(encoded["FilePathList"], serde_json::json!([]));
        for path in ["", "a", "docs/"] {
            input.file_path_list = vec![path.into()];
            assert!(input.validate().is_err());
        }
        input.is_share_all_files = false;
        for paths in [vec![], vec![""], vec!["a", "b"]] {
            input.file_path_list = paths.into_iter().map(str::to_owned).collect();
            assert!(input.validate().is_err());
        }
        input.file_path_list = vec!["docs/".into()];
        input.validate().unwrap();
        assert!(serde_json::to_value(input)
            .unwrap()
            .get("ShareAllFiles")
            .is_none());
    }

    #[test]
    fn metadata_preserves_an_optional_boolean_whole_space_scope() {
        for flag in [None, Some(false), Some(true)] {
            let mut encoded = serde_json::json!({"ShareID":"s"});
            if let Some(flag) = flag {
                encoded["ShareAllFiles"] = serde_json::json!(flag);
            }
            let info: ShareLinkInfo = serde_json::from_value(encoded.clone()).unwrap();
            assert_eq!(info.is_share_all_files, flag);
            assert_eq!(serde_json::to_value(info).unwrap(), encoded);
        }
    }
    use serde_json::json;

    #[test]
    fn share_info_preserves_missing_fields_and_unknown_additions() {
        let info: ShareLinkInfo =
            serde_json::from_value(json!({"ShareID":"s", "ShareURL":"http://inst.adriveapp.volces.com/share/s?pwd=delivery-secret", "FutureFlag":true})).unwrap();
        assert_eq!(info.share_id.as_deref(), Some("s"));
        assert_eq!(
            info.share_url.as_deref(),
            Some("http://inst.adriveapp.volces.com/share/s?pwd=delivery-secret")
        );
        assert!(!info.additional_fields.contains_key("ShareURL"));
        assert_eq!(info.require_login, None);
        assert_eq!(info.additional_fields.get("FutureFlag"), Some(&json!(true)));
    }

    #[test]
    fn share_info_accepts_file_paths_as_array_or_json_string() {
        for paths in [json!(["a", "b/"]), json!("[\"a\",\"b/\"]")] {
            let info: ShareLinkInfo =
                serde_json::from_value(json!({"FilePathList":paths})).unwrap();
            assert_eq!(info.file_path_list, Some(vec!["a".into(), "b/".into()]));
        }
        assert!(
            serde_json::from_value::<ShareLinkInfo>(json!({"FilePathList":"not json"})).is_err()
        );
    }

    #[test]
    fn partial_updates_serialize_explicit_zero_false_and_empty_only() {
        let input = UpdateShareLinkInput {
            instance_id: "i".into(),
            share_id: "s".into(),
            updated_at: 42,
            expiration_sec: Some(0),
            share_pwd: Some(String::new()),
            require_login: Some(false),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(input).unwrap(),
            json!({"ShareID":"s", "UpdatedAt":42,
            "ExpirationSec":0, "SharePwd":"", "RequireLogin":false})
        );
    }

    #[test]
    fn share_validation_rejects_multiple_roots_and_unsupported_permission() {
        let mut input = CreateShareLinkInput {
            instance_id: "i".into(),
            space_id: "s".into(),
            file_path_list: vec!["a".into()],
            ..Default::default()
        };
        assert!(input.validate().is_ok());
        input.file_path_list.push("b".into());
        assert!(input.validate().is_err());
        input.file_path_list.truncate(1);
        input.permission = Some("edit".into());
        assert!(input.validate().is_err());
    }

    #[test]
    fn search_requires_creators_and_bounded_offset() {
        let mut input = SearchShareLinksInput {
            instance_id: "i".into(),
            ..Default::default()
        };
        assert!(input.validate().is_err());
        input.creators = vec!["u1".into(), "u2".into()];
        input.offset = Some(10000);
        assert!(input.validate().is_ok());
        assert_eq!(
            input
                .to_query_pairs()
                .iter()
                .filter(|(key, _)| key == "creators")
                .count(),
            2
        );
        input.offset = Some(10001);
        assert!(input.validate().is_err());
        input.offset = None;
        input.created_at_start = Some("yesterday".into());
        assert!(input.validate().is_err());
    }

    #[test]
    fn list_requires_exactly_prefix_or_root() {
        assert!(ShareListFilesInput::default().validate().is_err());
        let mut input = ShareListFilesInput {
            list_root: Some(true),
            ..Default::default()
        };
        assert!(input.validate().is_ok());
        input.prefix = Some("folder/".into());
        assert!(input.validate().is_err());
        input.list_root = None;
        assert!(input.validate().is_ok());
        input.prefix = Some(String::new());
        assert!(input.validate().is_err());
    }

    #[test]
    fn search_debug_hides_unknown_sensitive_fields() {
        let output: SearchShareLinksOutput =
            serde_json::from_value(json!({"Items":[],"FutureToken":"future-secret"})).unwrap();
        assert!(!format!("{output:?}").contains("future-secret"));
    }

    #[test]
    fn debug_never_prints_response_password_token_or_extra() {
        let info: ShareLinkInfo =
            serde_json::from_value(json!({"SharePwd":"password-secret", "ShareURL":"https://pages.test/share/s?pwd=url-secret", "Extra":"token-secret"}))
                .unwrap();
        assert!(!format!("{info:?}").contains("secret"));
        let token = ShareAccessTokenOutput {
            access_token: "token-secret".into(),
            expires_in: 60,
        };
        assert!(!format!("{token:?}").contains("secret"));
    }
}
