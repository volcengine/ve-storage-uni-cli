//! Chunk API 请求、校验与响应；不包含索引管理操作。
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::{Error, HasResponseInfo, Result};
use super::types::ResponseInfo;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChunkQuery {
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchChunkInput {
    pub space_ids: [String; 1],
    pub folder_path: String,
    pub recursive: bool,
    pub search_query: [ChunkQuery; 1],
    pub limit: u32,
}

impl SearchChunkInput {
    pub fn validate(&self) -> Result<()> {
        validate_component(&self.space_ids[0], "SpaceId")?;
        let text = self.search_query[0].text.trim();
        if text.is_empty() || text.len() > 100 {
            return Err(Error::Client(
                "--text must be nonempty and at most 100 UTF-8 bytes after trimming".into(),
            ));
        }
        if !(1..=100).contains(&self.limit) {
            return Err(Error::Client("--limit must be between 1 and 100".into()));
        }
        validate_path(&self.folder_path, true)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChunkRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ReadChunkInput {
    pub space_id: String,
    pub file_path: String,
    #[serde(rename = "ETag")]
    pub etag: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chunk_ordinals: Vec<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_range: Option<ChunkRange>,
}

impl ReadChunkInput {
    pub fn validate(&self) -> Result<()> {
        validate_component(&self.space_id, "SpaceId")?;
        validate_path(&self.file_path, false)?;
        if self.etag.trim().is_empty() || self.etag.chars().any(char::is_control) {
            return Err(Error::Client(
                "--etag must contain the unchanged file version from a search hit".into(),
            ));
        }
        match (&self.chunk_range, self.chunk_ordinals.is_empty()) {
            (Some(range), true)
                if range.end <= i64::MAX as u64
                    && range.end >= range.start
                    && range.end - range.start < 20 =>
            {
                Ok(())
            }
            (None, false) if self.chunk_ordinals.len() <= 20 => {
                let mut seen = std::collections::HashSet::new();
                if self
                    .chunk_ordinals
                    .iter()
                    .all(|ordinal| *ordinal <= i64::MAX as u64 && seen.insert(ordinal))
                {
                    Ok(())
                } else {
                    Err(Error::Client(
                        "--ordinals must be unique nonnegative int64 values".into(),
                    ))
                }
            }
            _ => Err(Error::Client(
                "select exactly one of --ordinals or --range, with 1 through 20 chunks".into(),
            )),
        }
    }
}

pub(crate) fn validate_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/\\%?#:@".contains(c))
    {
        return Err(Error::Client(format!(
            "{label} must be a nonempty ID path segment"
        )));
    }
    Ok(())
}

pub(crate) fn instance_endpoint(endpoint: &str, instance: &str) -> Result<String> {
    validate_component(instance, "InstanceId")?;
    let mut url = reqwest::Url::parse(endpoint)
        .map_err(|_| Error::Client("invalid Chunk resource endpoint".into()))?;
    let host = url.host_str().unwrap_or_default();
    if host == "idsapi.volces.com" {
        url.set_host(Some(&format!("{instance}.idsapi.volces.com")))
            .map_err(|_| Error::Client("invalid instance resource host".into()))?;
    } else if let Some(configured_instance) = host.strip_suffix(".idsapi.volces.com") {
        if configured_instance != instance {
            return Err(Error::Client(
                "Chunk endpoint belongs to a different Instance".into(),
            ));
        }
    }
    // 自定义接入地址保持原样；实例身份仍由每次请求的 OAuth scope 校验。
    Ok(url.to_string().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_routes_use_the_instance_business_origin() {
        assert_eq!(
            instance_endpoint("https://idsapi.volces.com", "inst").unwrap(),
            "https://inst.idsapi.volces.com"
        );
        assert_eq!(
            instance_endpoint("https://inst.idsapi.volces.com", "inst").unwrap(),
            "https://inst.idsapi.volces.com"
        );
        assert!(instance_endpoint("https://other.idsapi.volces.com", "inst").is_err());
        assert_eq!(
            instance_endpoint("http://127.0.0.1:9000", "inst").unwrap(),
            "http://127.0.0.1:9000"
        );
        assert_eq!(
            instance_endpoint("https://custom.example.test/gateway/", "inst").unwrap(),
            "https://custom.example.test/gateway"
        );
    }
}

fn validate_path(path: &str, folder: bool) -> Result<()> {
    if folder && path == "/" {
        return Ok(());
    }
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/') != folder
        || path.chars().any(char::is_control)
        || path
            .trim_end_matches('/')
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(Error::Client(
            (if folder {
                "search requires a Space-relative directory ending in /, or the Space root"
            } else {
                "read requires one exact Space-relative file path"
            })
            .into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChunkOutput {
    // 保留服务返回的片段字段，不在客户端丢弃正文或未来扩展字段。
    pub items: Vec<Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
    #[serde(skip)]
    pub response_info: ResponseInfo,
}

impl HasResponseInfo for ChunkOutput {
    fn set_response_info(&mut self, info: ResponseInfo) {
        self.response_info = info;
    }
}
