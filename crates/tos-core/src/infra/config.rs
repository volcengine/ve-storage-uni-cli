/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! 统一配置文件模型（对应设计文档 7.3 "统一文件 × 独立入口"）。
//!
//! 磁盘上的 TOML 结构（`~/.tos/config.toml`）：
//!
//! ```toml
//! [default]
//! region = "cn-beijing"
//! access_key_id = "ENC:..."
//! secret_access_key = "ENC:..."
//!
//! [default.tos]
//! endpoint = "tos-cn-beijing.volces.com"
//! control_endpoint = "tos-control-cn-beijing.volces.com"
//! checkpoint_dir = "~/.tos/checkpoints"
//! batch_report_dir = "~/.tos/reports"
//! batch_report_format = "csv"
//! progress_enabled = true
//!
//! [default.tosvector]
//! endpoint = "tosvectors-cn-beijing.volces.com"
//!
//! [default.tostable]
//! endpoint = "tostables-cn-beijing.volces.com"
//!
//! [default.adrive]
//! endpoint = "..."
//! account_id = "2100000001"
//! default_instance = "inst-1"
//! default_space = "space-1"
//!
//! [staging]
//! region = "ap-southeast-1"
//! ```
//!
//! 读取时采用 "先 shared，再按二进制覆盖" 的继承规则：
//! 通过 [`ConfigFile::get_effective_profile`] 计算最终生效的配置。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::agent::error::CliError;
use crate::infra::crypto;

/// 支持的二进制标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binary {
    Tos,
    VeTos,
    TosVector,
    TosTable,
    Adrive,
}

impl Binary {
    pub fn as_str(self) -> &'static str {
        match self {
            Binary::Tos => "tos",
            Binary::VeTos => "ve-tos",
            Binary::TosVector => "tosvector",
            Binary::TosTable => "tostable",
            Binary::Adrive => "adrive",
        }
    }

    pub fn parse(s: &str) -> Option<Binary> {
        match s {
            "tos" => Some(Binary::Tos),
            "ve-tos" | "ve_tos" | "vetos" => Some(Binary::VeTos),
            "tosvector" | "tosvectors" => Some(Binary::TosVector),
            "tostable" | "tostables" => Some(Binary::TosTable),
            "adrive" => Some(Binary::Adrive),
            _ => None,
        }
    }
}

/// 敏感字段字段名集合（需要加密存储）。
// [Review Fix #m5] Canonical list of sensitive credential field name needles.
// Stored as lowercased ASCII; `is_secret_field` lowercases input and matches
// by substring so spellings like `accessKeyId`, `AccessKeyId`, `access-key-id`,
// `RocketmqAccessKeyId`, and presigned URL signature query params are all
// covered without exhaustively listing every casing.
const SECRET_FIELDS: &[&str] = &[
    "access_key_id",
    "secret_access_key",
    "security_token",
    "accesskeyid",
    "secretaccesskey",
    "securitytoken",
    "access-key-id",
    "secret-access-key",
    "security-token",
    "session_token",
    "sessiontoken",
    "session-token",
    "access_token",
    "accesstoken",
    "access-token",
    "refresh_token",
    "refreshtoken",
    "refresh-token",
    "client_secret",
    "clientsecret",
    "client-secret",
    "device_code",
    "devicecode",
    "device-code",
    "password",
    "passwd",
    "x-amz-signature",
    "x-tos-signature",
    "x-amz-credential",
    "x-tos-credential",
];
pub const DEFAULT_TOS_CHECKPOINT_DIR: &str = "~/.tos/checkpoints";
pub const DEFAULT_TOS_BATCH_REPORT_DIR: &str = "~/.tos/reports";
pub const DEFAULT_TOS_BATCH_REPORT_FORMAT: &str = "csv";
pub const DEFAULT_TOS_PROGRESS_ENABLED: bool = true;
pub const DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD: &str = "20MB";
pub const DEFAULT_BATCH_CONCURRENCY: usize = 16;
pub const DEFAULT_LIST_CONCURRENCY: usize = 4;
pub const DEFAULT_MULTIPART_CONCURRENCY: usize = 4;
pub const DEFAULT_PROGRESS_GRANULARITY: &str = "part";
pub const DEFAULT_OVERWRITE_STRATEGY: &str = "force";
pub const DEFAULT_HTTP_MAX_RETRY_COUNT: u32 = 3;
pub const DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS: u64 = 10;
pub const DEFAULT_HTTP_MAX_CONNECTIONS: usize = 100;

fn is_secret_field(name: &str) -> bool {
    // [Review Fix #m5] Case-insensitive substring match so RocketMQ-style
    // payloads (e.g. `RocketmqAccessKeyId`, `Mns.SecretAccessKey`) are also
    // redacted without bloating SECRET_FIELDS with every nesting variant.
    let lower = name.to_ascii_lowercase();
    SECRET_FIELDS
        .iter()
        .any(|needle| lower == *needle || lower.contains(needle))
}

// ---------------------------------------------------------------------------
// 二进制专属覆盖
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TosOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addr_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_report_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_report_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_threshold: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multipart_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_granularity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overwrite_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_count: Option<u32>,
    #[serde(alias = "request_timeout", skip_serializing_if = "Option::is_none")]
    pub requesttimeout: Option<u64>,
    #[serde(alias = "connect_timeout", skip_serializing_if = "Option::is_none")]
    pub connecttimeout: Option<u64>,
    #[serde(alias = "max_connections", skip_serializing_if = "Option::is_none")]
    pub maxconnections: Option<usize>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TosVectorOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_count: Option<u32>,
    #[serde(alias = "request_timeout", skip_serializing_if = "Option::is_none")]
    pub requesttimeout: Option<u64>,
    #[serde(alias = "connect_timeout", skip_serializing_if = "Option::is_none")]
    pub connecttimeout: Option<u64>,
    #[serde(alias = "max_connections", skip_serializing_if = "Option::is_none")]
    pub maxconnections: Option<usize>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TosTableOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_count: Option<u32>,
    #[serde(alias = "request_timeout", skip_serializing_if = "Option::is_none")]
    pub requesttimeout: Option<u64>,
    #[serde(alias = "connect_timeout", skip_serializing_if = "Option::is_none")]
    pub connecttimeout: Option<u64>,
    #[serde(alias = "max_connections", skip_serializing_if = "Option::is_none")]
    pub maxconnections: Option<usize>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdriveOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    // [Review Fix #1] This public field is an Authorization Server origin,
    // not the ADrive Resource Server endpoint above.
    /// OAuth Authorization Server base URL used for ADrive login.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_report_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_report_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_threshold: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multipart_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_granularity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overwrite_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_count: Option<u32>,
    #[serde(alias = "request_timeout", skip_serializing_if = "Option::is_none")]
    pub requesttimeout: Option<u64>,
    #[serde(alias = "connect_timeout", skip_serializing_if = "Option::is_none")]
    pub connecttimeout: Option<u64>,
    #[serde(alias = "max_connections", skip_serializing_if = "Option::is_none")]
    pub maxconnections: Option<usize>,
}

// ---------------------------------------------------------------------------
// Profile（shared + 四个可选 override）
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    // shared 字段直接放在根级
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// 仅运行时使用；PSM 配置只允许出现在 TOS 专属子表或 BYTE_TOS_* 环境变量。
    #[serde(skip)]
    pub psm: Option<String>,
    #[serde(skip)]
    pub idc: Option<String>,
    #[serde(skip)]
    pub cluster: Option<String>,
    #[serde(skip)]
    pub addr_family: Option<String>,
    /// 仅运行时使用；磁盘配置中的 `control_endpoint` 只允许出现在 TOS 专属子表。
    #[serde(skip)]
    pub control_endpoint: Option<String>,
    #[serde(skip)]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_report_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_report_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_threshold: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multipart_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_granularity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overwrite_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_count: Option<u32>,
    #[serde(alias = "request_timeout", skip_serializing_if = "Option::is_none")]
    pub requesttimeout: Option<u64>,
    #[serde(alias = "connect_timeout", skip_serializing_if = "Option::is_none")]
    pub connecttimeout: Option<u64>,
    #[serde(alias = "max_connections", skip_serializing_if = "Option::is_none")]
    pub maxconnections: Option<usize>,

    // 二进制专属 override（对应 TOML 的 `[<profile>.<binary>]` 子表）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tos: Option<TosOverride>,
    #[serde(
        rename = "ve-tos",
        alias = "ve_tos",
        skip_serializing_if = "Option::is_none"
    )]
    pub ve_tos: Option<TosOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tosvector: Option<TosVectorOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tostable: Option<TosTableOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adrive: Option<AdriveOverride>,
}

impl Profile {
    fn clear_aksk_credentials(&mut self) {
        self.access_key_id = None;
        self.secret_access_key = None;
        self.security_token = None;
        for settings in [&mut self.tos, &mut self.ve_tos].into_iter().flatten() {
            settings.access_key_id = None;
            settings.secret_access_key = None;
            settings.security_token = None;
        }
        if let Some(settings) = self.tosvector.as_mut() {
            settings.access_key_id = None;
            settings.secret_access_key = None;
            settings.security_token = None;
        }
        if let Some(settings) = self.tostable.as_mut() {
            settings.access_key_id = None;
            settings.secret_access_key = None;
            settings.security_token = None;
        }
        if let Some(settings) = self.adrive.as_mut() {
            settings.access_key_id = None;
            settings.secret_access_key = None;
            settings.security_token = None;
        }
    }

    /// 从环境变量构建 Profile（shared 字段）。
    pub fn from_env() -> Self {
        Self::from_env_with_prefix("TOS", true)
    }

    /// Build a TOS environment profile without accessing credential variables.
    ///
    /// Resource, timeout, and runtime-control fields are retained while
    /// `TOS_ACCESS_KEY`, `TOS_SECRET_KEY`, and `TOS_SECURITY_TOKEN` are never
    /// queried. This is the environment source for credential-free auth modes.
    pub fn from_env_without_credentials() -> Self {
        Self::from_env_with_prefix("TOS", false)
    }

    /// 从 ByteCloud TOS 专属环境变量构建 Profile。
    pub fn from_byte_tos_env() -> Self {
        Self::from_byte_tos_env_with_credentials(true)
    }

    /// Build the BYTE_TOS environment profile without reading AKSK variables.
    ///
    /// Returns network, PSM, and runtime settings; credential fields are unset.
    pub fn from_byte_tos_env_without_credentials() -> Self {
        Self::from_byte_tos_env_with_credentials(false)
    }

    fn from_byte_tos_env_with_credentials(include_credentials: bool) -> Self {
        let mut profile = Self::from_env_with_prefix("BYTE_TOS", include_credentials);
        // [Review Fix #1] Blank BYTE_TOS_PSM must not activate IDC/cluster/addr_family.
        profile.psm = env_var("BYTE_TOS", "PSM").filter(|value| !value.trim().is_empty());
        if profile.psm.is_some() {
            profile.idc = env_var("BYTE_TOS", "IDC");
            profile.cluster = env_var("BYTE_TOS", "CLUSTER");
            profile.addr_family = env_var("BYTE_TOS", "ADDR_FAMILY");
        }
        profile
    }

    fn from_env_with_prefix(prefix: &str, include_credentials: bool) -> Self {
        // [Review Fix #4] Unified auth must not even query unselected
        // credential variables; clearing a fully populated profile is too late.
        let (access_key_id, secret_access_key, security_token) = if include_credentials {
            (
                env_var(prefix, "ACCESS_KEY"),
                env_var(prefix, "SECRET_KEY"),
                env_var(prefix, "SECURITY_TOKEN"),
            )
        } else {
            (None, None, None)
        };
        Self {
            region: env_var(prefix, "REGION"),
            access_key_id,
            secret_access_key,
            security_token,
            endpoint: env_var(prefix, "ENDPOINT"),
            psm: None,
            idc: None,
            cluster: None,
            addr_family: None,
            control_endpoint: env_var(prefix, "CONTROL_ENDPOINT"),
            account_id: env_var(prefix, "ACCOUNT_ID"),
            checkpoint_dir: env_var(prefix, "CHECKPOINT_DIR"),
            batch_report_dir: env_var(prefix, "BATCH_REPORT_DIR"),
            batch_report_format: env_var(prefix, "BATCH_REPORT_FORMAT"),
            progress_enabled: env_var(prefix, "PROGRESS_ENABLED")
                .and_then(|value| parse_bool(&value).ok()),
            checkpoint_threshold: env_var(prefix, "CHECKPOINT_THRESHOLD"),
            batch_concurrency: env_var_any(&[&env_key(prefix, "BATCH_CONCURRENCY")])
                .and_then(|value| parse_positive_usize_field(&value, "batch_concurrency").ok()),
            list_concurrency: env_var_any(&[&env_key(prefix, "LIST_CONCURRENCY")])
                .and_then(|value| parse_positive_usize_field(&value, "list_concurrency").ok()),
            multipart_concurrency: env_var_any(&[&env_key(prefix, "MULTIPART_CONCURRENCY")])
                .and_then(|value| parse_positive_usize_field(&value, "multipart_concurrency").ok()),
            progress_granularity: env_var(prefix, "PROGRESS_GRANULARITY"),
            overwrite_strategy: env_var(prefix, "OVERWRITE_STRATEGY"),
            max_retry_count: env_var_any(&[&env_key(prefix, "MAX_RETRY_COUNT")])
                .and_then(|value| parse_u32_field(&value, "max_retry_count").ok()),
            requesttimeout: env_var_any(&[
                &env_key(prefix, "REQUESTTIMEOUT"),
                &env_key(prefix, "REQUEST_TIMEOUT"),
            ])
            .and_then(|value| parse_positive_u64_field(&value, "requesttimeout").ok()),
            connecttimeout: env_var_any(&[
                &env_key(prefix, "CONNECTTIMEOUT"),
                &env_key(prefix, "CONNECT_TIMEOUT"),
            ])
            .and_then(|value| parse_positive_u64_field(&value, "connecttimeout").ok()),
            maxconnections: env_var_any(&[
                &env_key(prefix, "MAXCONNECTIONS"),
                &env_key(prefix, "MAX_CONNECTIONS"),
            ])
            .and_then(|value| parse_positive_usize_field(&value, "maxconnections").ok()),
            tos: None,
            ve_tos: None,
            tosvector: None,
            tostable: None,
            adrive: None,
        }
    }

    /// 合并两个 Profile 的 shared 字段（other 优先）。只合并根级字段。
    pub fn merge(&self, other: &Profile) -> Profile {
        Profile {
            region: other.region.clone().or_else(|| self.region.clone()),
            access_key_id: other
                .access_key_id
                .clone()
                .or_else(|| self.access_key_id.clone()),
            secret_access_key: other
                .secret_access_key
                .clone()
                .or_else(|| self.secret_access_key.clone()),
            security_token: other
                .security_token
                .clone()
                .or_else(|| self.security_token.clone()),
            endpoint: other.endpoint.clone().or_else(|| self.endpoint.clone()),
            psm: other.psm.clone().or_else(|| self.psm.clone()),
            idc: other.idc.clone().or_else(|| self.idc.clone()),
            cluster: other.cluster.clone().or_else(|| self.cluster.clone()),
            addr_family: other
                .addr_family
                .clone()
                .or_else(|| self.addr_family.clone()),
            control_endpoint: other
                .control_endpoint
                .clone()
                .or_else(|| self.control_endpoint.clone()),
            account_id: other.account_id.clone().or_else(|| self.account_id.clone()),
            checkpoint_dir: other
                .checkpoint_dir
                .clone()
                .or_else(|| self.checkpoint_dir.clone()),
            batch_report_dir: other
                .batch_report_dir
                .clone()
                .or_else(|| self.batch_report_dir.clone()),
            batch_report_format: other
                .batch_report_format
                .clone()
                .or_else(|| self.batch_report_format.clone()),
            progress_enabled: other.progress_enabled.or(self.progress_enabled),
            checkpoint_threshold: other
                .checkpoint_threshold
                .clone()
                .or_else(|| self.checkpoint_threshold.clone()),
            batch_concurrency: other.batch_concurrency.or(self.batch_concurrency),
            list_concurrency: other.list_concurrency.or(self.list_concurrency),
            multipart_concurrency: other.multipart_concurrency.or(self.multipart_concurrency),
            progress_granularity: other
                .progress_granularity
                .clone()
                .or_else(|| self.progress_granularity.clone()),
            overwrite_strategy: other
                .overwrite_strategy
                .clone()
                .or_else(|| self.overwrite_strategy.clone()),
            max_retry_count: other.max_retry_count.or(self.max_retry_count),
            requesttimeout: other.requesttimeout.or(self.requesttimeout),
            connecttimeout: other.connecttimeout.or(self.connecttimeout),
            maxconnections: other.maxconnections.or(self.maxconnections),
            tos: other.tos.clone().or_else(|| self.tos.clone()),
            ve_tos: other.ve_tos.clone().or_else(|| self.ve_tos.clone()),
            tosvector: other.tosvector.clone().or_else(|| self.tosvector.clone()),
            tostable: other.tostable.clone().or_else(|| self.tostable.clone()),
            adrive: other.adrive.clone().or_else(|| self.adrive.clone()),
        }
    }

    /// 遮蔽敏感字段后的 shared 视图（用于 `tos config show` 非溯源模式）。
    pub fn redacted(&self) -> Profile {
        Profile {
            region: self.region.clone(),
            access_key_id: self.access_key_id.as_ref().map(|s| mask_secret(s)),
            secret_access_key: self.secret_access_key.as_ref().map(|s| mask_secret(s)),
            security_token: self.security_token.as_ref().map(|s| mask_secret(s)),
            endpoint: self.endpoint.clone(),
            psm: self.psm.clone(),
            idc: self.idc.clone(),
            cluster: self.cluster.clone(),
            addr_family: self.addr_family.clone(),
            control_endpoint: self.control_endpoint.clone(),
            account_id: self.account_id.clone(),
            checkpoint_dir: self.checkpoint_dir.clone(),
            batch_report_dir: self.batch_report_dir.clone(),
            batch_report_format: self.batch_report_format.clone(),
            progress_enabled: self.progress_enabled,
            checkpoint_threshold: self.checkpoint_threshold.clone(),
            batch_concurrency: self.batch_concurrency,
            list_concurrency: self.list_concurrency,
            multipart_concurrency: self.multipart_concurrency,
            progress_granularity: self.progress_granularity.clone(),
            overwrite_strategy: self.overwrite_strategy.clone(),
            max_retry_count: self.max_retry_count,
            requesttimeout: self.requesttimeout,
            connecttimeout: self.connecttimeout,
            maxconnections: self.maxconnections,
            tos: self.tos.clone(),
            ve_tos: self.ve_tos.clone(),
            tosvector: self.tosvector.clone(),
            tostable: self.tostable.clone(),
            adrive: self.adrive.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// 生效视图：shared + 指定 binary override 合并后的结果（含每字段来源）
// ---------------------------------------------------------------------------

/// 某字段的来源 section 标签。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum FieldSource {
    /// 来自 shared profile，即 `[<profile>]`。
    Shared,
    /// 来自 binary override，即 `[<profile>.<binary>]`。
    BinaryOverride,
    /// 根据 data endpoint 自动推导。
    Derived,
    /// 来自独立的 credentials.toml。
    #[serde(rename = "credentials_file")]
    CredentialsFile,
    /// Selected by an explicit command-line option.
    #[serde(rename = "command_line")]
    CommandLine,
    /// Selected by an environment variable.
    #[serde(rename = "environment")]
    Environment,
    /// Selected by the compatibility default.
    #[serde(rename = "compatibility_default")]
    CompatibilityDefault,
    /// 未配置。
    Unset,
}

impl FieldSource {
    /// 渲染成展示标签，如 `← [default]` / `← [default.tos]` / `-`。
    pub fn label(&self, profile_name: &str, binary: &str) -> String {
        match self {
            FieldSource::Shared => format!("[{}]", profile_name),
            FieldSource::BinaryOverride => format!("[{}.{}]", profile_name, binary),
            FieldSource::Derived => "derived from endpoint".to_string(),
            FieldSource::CredentialsFile => "credentials_file".to_string(),
            FieldSource::CommandLine => "command_line".to_string(),
            FieldSource::Environment => "environment".to_string(),
            FieldSource::CompatibilityDefault => "compatibility_default".to_string(),
            FieldSource::Unset => "-".to_string(),
        }
    }
}

/// 某字段 + 来源的组合。
#[derive(Debug, Clone, Serialize)]
pub struct TracedField<T: Clone + Serialize> {
    pub value: Option<T>,
    pub source: FieldSource,
}

impl<T: Clone + Serialize> TracedField<T> {
    fn is_unset(field: &Self) -> bool {
        field.value.is_none() && field.source == FieldSource::Unset
    }

    fn shared(v: Option<T>) -> Self {
        TracedField {
            source: if v.is_some() {
                FieldSource::Shared
            } else {
                FieldSource::Unset
            },
            value: v,
        }
    }
    fn override_with(base: TracedField<T>, v: Option<T>) -> Self {
        match v {
            Some(x) => TracedField {
                value: Some(x),
                source: FieldSource::BinaryOverride,
            },
            None => base,
        }
    }
}

/// 某个 profile 在某个 binary 视角下的生效配置 + 来源溯源。
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveProfile {
    pub profile_name: String,
    pub binary: String,
    pub region: TracedField<String>,
    pub endpoint: TracedField<String>,
    /// adrive 专属：OAuth Authorization Server endpoint。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_endpoint: Option<TracedField<String>>,
    #[serde(skip_serializing_if = "TracedField::is_unset")]
    pub psm: TracedField<String>,
    #[serde(skip_serializing_if = "TracedField::is_unset")]
    pub idc: TracedField<String>,
    #[serde(skip_serializing_if = "TracedField::is_unset")]
    pub cluster: TracedField<String>,
    #[serde(skip_serializing_if = "TracedField::is_unset")]
    pub addr_family: TracedField<String>,
    #[serde(skip_serializing_if = "TracedField::is_unset")]
    pub control_endpoint: TracedField<String>,
    pub checkpoint_dir: TracedField<String>,
    pub batch_report_dir: TracedField<String>,
    pub batch_report_format: TracedField<String>,
    pub progress_enabled: TracedField<bool>,
    pub checkpoint_threshold: TracedField<String>,
    pub batch_concurrency: TracedField<usize>,
    pub list_concurrency: TracedField<usize>,
    pub multipart_concurrency: TracedField<usize>,
    pub progress_granularity: TracedField<String>,
    pub overwrite_strategy: TracedField<String>,
    pub max_retry_count: TracedField<u32>,
    pub requesttimeout: TracedField<u64>,
    pub connecttimeout: TracedField<u64>,
    pub maxconnections: TracedField<usize>,
    pub access_key_id: TracedField<String>,
    pub secret_access_key: TracedField<String>,
    pub security_token: TracedField<String>,
    // [Review Fix #1] Authentication mode is exposed only for the two binaries
    // whose schemas support explicit mode selection.
    /// ve-tos 与 adrive 专属：认证策略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<TracedField<String>>,
    /// adrive 专属：OAuth access token，仅用于脱敏配置检查。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<TracedField<String>>,
    /// adrive 专属：OAuth refresh token，仅用于脱敏配置检查。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<TracedField<String>>,
    /// adrive 专属：账号 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<TracedField<String>>,
    /// adrive 专属：默认实例。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_instance: Option<TracedField<String>>,
    /// adrive 专属：默认空间。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_space: Option<TracedField<String>>,
}

/// Overlay credentials-file fields onto an effective profile for inspection.
pub fn overlay_effective_credentials(
    effective: &mut EffectiveProfile,
    credentials: &crate::infra::credentials::StoredAkskCredentials,
) {
    if let Some(value) = credentials.access_key_id.as_ref() {
        effective.access_key_id = TracedField {
            value: Some(value.clone()),
            source: FieldSource::CredentialsFile,
        };
    }
    if let Some(value) = credentials.secret_access_key.as_ref() {
        effective.secret_access_key = TracedField {
            value: Some(value.clone()),
            source: FieldSource::CredentialsFile,
        };
    }
    if let Some(value) = credentials.security_token.as_ref() {
        effective.security_token = TracedField {
            value: Some(value.clone()),
            source: FieldSource::CredentialsFile,
        };
    }
}

/// Overlay ADrive OAuth fields for redacted configuration inspection.
pub fn overlay_effective_oauth_credentials(
    effective: &mut EffectiveProfile,
    credentials: &crate::infra::credentials::StoredOAuthCredentials,
) {
    // [Review Fix #14] Keep OAuth credentials visible to config inspection
    // without folding them into the legacy SDK profile consumed at runtime.
    if let Some(value) = credentials.access_token.as_ref() {
        effective.access_token = Some(TracedField {
            value: Some(value.clone()),
            source: FieldSource::CredentialsFile,
        });
    }
    if let Some(value) = credentials.refresh_token.as_ref() {
        effective.refresh_token = Some(TracedField {
            value: Some(value.clone()),
            source: FieldSource::CredentialsFile,
        });
    }
}

impl EffectiveProfile {
    /// 折叠为不带溯源的 flat Profile，供下游 SDK/client 消费。
    pub fn into_flat_profile(&self) -> Profile {
        Profile {
            region: self.region.value.clone(),
            access_key_id: self.access_key_id.value.clone(),
            secret_access_key: self.secret_access_key.value.clone(),
            security_token: self.security_token.value.clone(),
            endpoint: self.endpoint.value.clone(),
            psm: self.psm.value.clone(),
            idc: self.idc.value.clone(),
            cluster: self.cluster.value.clone(),
            addr_family: self.addr_family.value.clone(),
            control_endpoint: self.control_endpoint.value.clone(),
            account_id: None,
            checkpoint_dir: self.checkpoint_dir.value.clone(),
            batch_report_dir: self.batch_report_dir.value.clone(),
            batch_report_format: self.batch_report_format.value.clone(),
            progress_enabled: self.progress_enabled.value,
            checkpoint_threshold: self.checkpoint_threshold.value.clone(),
            batch_concurrency: self.batch_concurrency.value,
            list_concurrency: self.list_concurrency.value,
            multipart_concurrency: self.multipart_concurrency.value,
            progress_granularity: self.progress_granularity.value.clone(),
            overwrite_strategy: self.overwrite_strategy.value.clone(),
            max_retry_count: self.max_retry_count.value,
            requesttimeout: self.requesttimeout.value,
            connecttimeout: self.connecttimeout.value,
            maxconnections: self.maxconnections.value,
            tos: None,
            ve_tos: None,
            tosvector: None,
            tostable: None,
            adrive: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ConfigFile：磁盘容器（多个 profile）
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct ConfigFile {
    /// profile_name → Profile（对应 TOML 根级 `[default]` / `[staging]` 等 section）。
    #[serde(flatten)]
    pub profiles: BTreeMap<String, Profile>,
}

impl ConfigFile {
    /// 配置目录，默认为 `$HOME/.tos/`。
    pub fn config_dir() -> PathBuf {
        dirs_next::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".tos")
    }

    /// 返回指定配置文件的配置目录。
    ///
    /// 该目录用于保存配置文件以及本地加密密钥；当 `path` 没有父目录时，
    /// 返回当前目录。
    pub fn config_dir_from_path(path: &Path) -> PathBuf {
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// 配置文件路径，默认 `$HOME/.tos/config.toml`。
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// 返回显式配置路径或默认配置路径。
    ///
    /// `path` 为 `Some` 时原样使用该路径；为 `None` 时使用
    /// `$HOME/.tos/config.toml`。
    pub fn config_path_from(path: Option<&Path>) -> PathBuf {
        path.map(Path::to_path_buf)
            .unwrap_or_else(Self::config_path)
    }

    /// 从默认路径加载；不存在则返回空 ConfigFile。敏感字段读出时保留 `ENC:` 原样。
    pub fn load() -> Result<ConfigFile, CliError> {
        Self::load_from(&Self::config_path())
    }

    /// 从指定路径加载。
    pub fn load_from(path: &Path) -> Result<ConfigFile, CliError> {
        if !path.exists() {
            return Ok(ConfigFile::default());
        }
        let content = std::fs::read_to_string(path).map_err(|e| {
            CliError::ConfigMissing(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;
        let config: ConfigFile = toml::from_str(&content).map_err(|_error| {
            // [Review Fix #6] TOML parser diagnostics quote the offending
            // source line, which can contain a plaintext credential.
            CliError::ValidationError(format!(
                "Failed to parse config file {}; validate its TOML syntax and schema",
                path.display()
            ))
        })?;
        Ok(config)
    }

    /// 将当前 ConfigFile 持久化到默认路径；敏感字段若为明文自动加密为 `ENC:...`。
    pub fn save(&self) -> Result<(), CliError> {
        self.save_to(&Self::config_dir(), &Self::config_path())
    }

    /// 将当前 ConfigFile 持久化到指定配置文件路径。
    ///
    /// 文件父目录会在缺失时自动创建，并作为本地加密密钥目录。
    pub fn save_to_path(&self, path: &Path) -> Result<(), CliError> {
        self.save_to(&Self::config_dir_from_path(path), path)
    }

    /// 将当前 ConfigFile 持久化到指定目录/文件。
    pub fn save_to(&self, config_dir: &Path, path: &Path) -> Result<(), CliError> {
        if !config_dir.exists() {
            std::fs::create_dir_all(config_dir).map_err(|e| {
                CliError::Io(std::io::Error::new(
                    e.kind(),
                    format!(
                        "Failed to create config dir {}: {}",
                        config_dir.display(),
                        e
                    ),
                ))
            })?;
        }

        // 复制一份自身，对明文敏感字段进行即时加密
        let mut to_write = self.clone();
        let key = crypto::load_or_init_key(config_dir)?;
        for profile in to_write.profiles.values_mut() {
            encrypt_profile_in_place(profile, &key)?;
        }

        let content = toml::to_string_pretty(&to_write)
            .map_err(|e| CliError::ValidationError(format!("Failed to serialize config: {}", e)))?;
        super::atomic_file::write_owner_only_atomic(path, content.as_bytes())
    }

    /// 获取 profile（只读）。
    pub fn get_profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    /// 获取 profile（可写），不存在则创建空 profile。
    pub fn get_or_insert_profile(&mut self, name: &str) -> &mut Profile {
        self.profiles
            .entry(name.to_string())
            .or_insert_with(Profile::default)
    }

    /// 按 key 写入值。支持的 key 格式：
    ///   - `region`                       → [default] 的 shared 字段
    ///   - `default.region`               → [default] 的 shared 字段
    ///   - `default.tos.endpoint`         → [default.tos].endpoint
    ///   - `staging.adrive.account_id`    → [staging.adrive].account_id
    pub fn set_by_path(&mut self, path: &[&str], value: &str) -> Result<(), CliError> {
        match path.len() {
            1 => {
                // 等价于 default.<field>
                let profile = self.get_or_insert_profile("default");
                set_shared_field(profile, path[0], value)
            }
            2 => {
                // 可能是 <profile>.<field> 或 <profile>.<binary>?——只认 shared
                let profile = self.get_or_insert_profile(path[0]);
                set_shared_field(profile, path[1], value)
            }
            3 => {
                // <profile>.<binary>.<field>
                let profile = self.get_or_insert_profile(path[0]);
                set_binary_field(profile, path[1], path[2], value)
            }
            _ => Err(CliError::ValidationError(format!(
                "Invalid config key path: {:?} (expected 1-3 segments)",
                path
            ))),
        }
    }

    /// 计算某个 profile 在某个 binary 视角下的生效配置（shared ← binary override）。
    pub fn get_effective_profile(
        &self,
        profile_name: &str,
        binary: Binary,
    ) -> Result<EffectiveProfile, CliError> {
        self.get_effective_profile_in_dir(profile_name, binary, &Self::config_dir())
    }

    /// 计算某个 profile 在某个 binary 视角下的生效配置，并使用指定配置目录解密敏感字段。
    ///
    /// `config_dir` 必须与保存该配置文件时使用的配置目录一致，否则 `ENC:`
    /// 敏感字段无法用对应的本地密钥解密。
    pub fn get_effective_profile_in_dir(
        &self,
        profile_name: &str,
        binary: Binary,
        config_dir: &Path,
    ) -> Result<EffectiveProfile, CliError> {
        let profile = self.profiles.get(profile_name).ok_or_else(|| {
            CliError::ConfigMissing(format!(
                "Profile '{}' not found in config (available: {:?})",
                profile_name,
                self.profiles.keys().collect::<Vec<_>>()
            ))
        })?;
        ensure_tos_namespace_isolated(profile_name, profile, binary)?;

        // 1) 从 shared 起步
        let mut region = TracedField::shared(profile.region.clone());
        let mut endpoint = TracedField::shared(profile.endpoint.clone());
        let mut psm = TracedField::shared(profile.psm.clone());
        let mut idc = TracedField::shared(profile.idc.clone());
        let mut cluster = TracedField::shared(profile.cluster.clone());
        let mut addr_family = TracedField::shared(profile.addr_family.clone());
        let mut control_endpoint = TracedField::shared(profile.control_endpoint.clone());
        let mut checkpoint_dir = TracedField::shared(profile.checkpoint_dir.clone());
        let mut batch_report_dir = TracedField::shared(profile.batch_report_dir.clone());
        let mut batch_report_format = TracedField::shared(profile.batch_report_format.clone());
        let mut progress_enabled = TracedField::shared(profile.progress_enabled);
        let mut checkpoint_threshold = TracedField::shared(profile.checkpoint_threshold.clone());
        let mut batch_concurrency = TracedField::shared(profile.batch_concurrency);
        let mut list_concurrency = TracedField::shared(profile.list_concurrency);
        let mut multipart_concurrency = TracedField::shared(profile.multipart_concurrency);
        let mut progress_granularity = TracedField::shared(profile.progress_granularity.clone());
        let mut overwrite_strategy = TracedField::shared(profile.overwrite_strategy.clone());
        let mut max_retry_count = TracedField::shared(profile.max_retry_count);
        let mut requesttimeout = TracedField::shared(profile.requesttimeout);
        let mut connecttimeout = TracedField::shared(profile.connecttimeout);
        let mut maxconnections = TracedField::shared(profile.maxconnections);
        // Placeholder credentials written by `config init` (e.g. "<YOUR_ACCESS_KEY_ID>")
        // are not real values; treat them as unset so `config show` reports them
        // as "-" / not configured instead of a masked pseudo-secret.
        let mut access_key_id =
            TracedField::shared(strip_placeholder(profile.access_key_id.clone()));
        let mut secret_access_key =
            TracedField::shared(strip_placeholder(profile.secret_access_key.clone()));
        let mut security_token =
            TracedField::shared(strip_placeholder(profile.security_token.clone()));

        let mut auth_endpoint: Option<TracedField<String>> = None;
        let mut account_id: Option<TracedField<String>> = None;
        let mut default_instance: Option<TracedField<String>> = None;
        let mut default_space: Option<TracedField<String>> = None;
        let mut auth_mode: Option<TracedField<String>> = None;

        // 2) 按 binary 覆盖
        match binary {
            Binary::Tos | Binary::VeTos => {
                let mut tos_account_id = TracedField::shared(profile.account_id.clone());
                let tos_override = if binary == Binary::Tos {
                    profile.tos.as_ref()
                } else {
                    profile.ve_tos.as_ref()
                };
                {
                    let configured_auth_mode = tos_override
                        .and_then(|tos_override| tos_override.auth_mode.as_deref())
                        .map(|value| normalize_tos_auth_mode(value, binary == Binary::VeTos))
                        .transpose()?;
                    auth_mode = Some(TracedField {
                        source: if configured_auth_mode.is_some() {
                            FieldSource::BinaryOverride
                        } else {
                            FieldSource::Unset
                        },
                        value: configured_auth_mode,
                    });
                }
                if let Some(o) = tos_override {
                    region = TracedField::override_with(region, o.region.clone());
                    endpoint = TracedField::override_with(endpoint, o.endpoint.clone());
                    psm = TracedField::override_with(psm, o.psm.clone());
                    idc = TracedField::override_with(idc, o.idc.clone());
                    cluster = TracedField::override_with(cluster, o.cluster.clone());
                    addr_family = TracedField::override_with(addr_family, o.addr_family.clone());
                    control_endpoint =
                        TracedField::override_with(control_endpoint, o.control_endpoint.clone());
                    tos_account_id =
                        TracedField::override_with(tos_account_id, o.account_id.clone());
                    checkpoint_dir =
                        TracedField::override_with(checkpoint_dir, o.checkpoint_dir.clone());
                    batch_report_dir =
                        TracedField::override_with(batch_report_dir, o.batch_report_dir.clone());
                    batch_report_format = TracedField::override_with(
                        batch_report_format,
                        o.batch_report_format.clone(),
                    );
                    progress_enabled =
                        TracedField::override_with(progress_enabled, o.progress_enabled);
                    checkpoint_threshold = TracedField::override_with(
                        checkpoint_threshold,
                        o.checkpoint_threshold.clone(),
                    );
                    batch_concurrency =
                        TracedField::override_with(batch_concurrency, o.batch_concurrency);
                    list_concurrency =
                        TracedField::override_with(list_concurrency, o.list_concurrency);
                    multipart_concurrency =
                        TracedField::override_with(multipart_concurrency, o.multipart_concurrency);
                    progress_granularity = TracedField::override_with(
                        progress_granularity,
                        o.progress_granularity.clone(),
                    );
                    overwrite_strategy = TracedField::override_with(
                        overwrite_strategy,
                        o.overwrite_strategy.clone(),
                    );
                    max_retry_count =
                        TracedField::override_with(max_retry_count, o.max_retry_count);
                    requesttimeout = TracedField::override_with(requesttimeout, o.requesttimeout);
                    connecttimeout = TracedField::override_with(connecttimeout, o.connecttimeout);
                    maxconnections = TracedField::override_with(maxconnections, o.maxconnections);
                    access_key_id =
                        TracedField::override_with(access_key_id, o.access_key_id.clone());
                    secret_access_key =
                        TracedField::override_with(secret_access_key, o.secret_access_key.clone());
                    security_token =
                        TracedField::override_with(security_token, o.security_token.clone());
                }
                account_id = Some(tos_account_id);
            }
            Binary::TosVector => {
                if let Some(o) = &profile.tosvector {
                    region = TracedField::override_with(region, o.region.clone());
                    endpoint = TracedField::override_with(endpoint, o.endpoint.clone());
                    access_key_id =
                        TracedField::override_with(access_key_id, o.access_key_id.clone());
                    secret_access_key =
                        TracedField::override_with(secret_access_key, o.secret_access_key.clone());
                    security_token =
                        TracedField::override_with(security_token, o.security_token.clone());
                    max_retry_count =
                        TracedField::override_with(max_retry_count, o.max_retry_count);
                    requesttimeout = TracedField::override_with(requesttimeout, o.requesttimeout);
                    connecttimeout = TracedField::override_with(connecttimeout, o.connecttimeout);
                    maxconnections = TracedField::override_with(maxconnections, o.maxconnections);
                }
            }
            Binary::TosTable => {
                if let Some(o) = &profile.tostable {
                    region = TracedField::override_with(region, o.region.clone());
                    endpoint = TracedField::override_with(endpoint, o.endpoint.clone());
                    access_key_id =
                        TracedField::override_with(access_key_id, o.access_key_id.clone());
                    secret_access_key =
                        TracedField::override_with(secret_access_key, o.secret_access_key.clone());
                    security_token =
                        TracedField::override_with(security_token, o.security_token.clone());
                    max_retry_count =
                        TracedField::override_with(max_retry_count, o.max_retry_count);
                    requesttimeout = TracedField::override_with(requesttimeout, o.requesttimeout);
                    connecttimeout = TracedField::override_with(connecttimeout, o.connecttimeout);
                    maxconnections = TracedField::override_with(maxconnections, o.maxconnections);
                }
            }
            Binary::Adrive => {
                if let Some(o) = &profile.adrive {
                    let configured_auth_mode = o
                        .auth_mode
                        .as_deref()
                        .map(normalize_adrive_auth_mode)
                        .transpose()?;
                    auth_mode = Some(TracedField {
                        source: if configured_auth_mode.is_some() {
                            FieldSource::BinaryOverride
                        } else {
                            FieldSource::Unset
                        },
                        value: configured_auth_mode,
                    });
                    region = TracedField::override_with(region, o.region.clone());
                    endpoint = TracedField::override_with(endpoint, o.endpoint.clone());
                    auth_endpoint = Some(TracedField {
                        source: if o.auth_endpoint.is_some() {
                            FieldSource::BinaryOverride
                        } else {
                            FieldSource::Unset
                        },
                        value: o.auth_endpoint.clone(),
                    });
                    checkpoint_dir =
                        TracedField::override_with(checkpoint_dir, o.checkpoint_dir.clone());
                    batch_report_dir =
                        TracedField::override_with(batch_report_dir, o.batch_report_dir.clone());
                    batch_report_format = TracedField::override_with(
                        batch_report_format,
                        o.batch_report_format.clone(),
                    );
                    progress_enabled =
                        TracedField::override_with(progress_enabled, o.progress_enabled);
                    checkpoint_threshold = TracedField::override_with(
                        checkpoint_threshold,
                        o.checkpoint_threshold.clone(),
                    );
                    batch_concurrency =
                        TracedField::override_with(batch_concurrency, o.batch_concurrency);
                    list_concurrency =
                        TracedField::override_with(list_concurrency, o.list_concurrency);
                    multipart_concurrency =
                        TracedField::override_with(multipart_concurrency, o.multipart_concurrency);
                    progress_granularity = TracedField::override_with(
                        progress_granularity,
                        o.progress_granularity.clone(),
                    );
                    overwrite_strategy = TracedField::override_with(
                        overwrite_strategy,
                        o.overwrite_strategy.clone(),
                    );
                    max_retry_count =
                        TracedField::override_with(max_retry_count, o.max_retry_count);
                    requesttimeout = TracedField::override_with(requesttimeout, o.requesttimeout);
                    connecttimeout = TracedField::override_with(connecttimeout, o.connecttimeout);
                    maxconnections = TracedField::override_with(maxconnections, o.maxconnections);
                    access_key_id =
                        TracedField::override_with(access_key_id, o.access_key_id.clone());
                    secret_access_key =
                        TracedField::override_with(secret_access_key, o.secret_access_key.clone());
                    security_token =
                        TracedField::override_with(security_token, o.security_token.clone());
                    account_id = Some(TracedField {
                        source: if o.account_id.is_some() {
                            FieldSource::BinaryOverride
                        } else {
                            FieldSource::Unset
                        },
                        value: o.account_id.clone(),
                    });
                    default_instance = Some(TracedField {
                        source: if o.default_instance.is_some() {
                            FieldSource::BinaryOverride
                        } else {
                            FieldSource::Unset
                        },
                        value: o.default_instance.clone(),
                    });
                    default_space = Some(TracedField {
                        source: if o.default_space.is_some() {
                            FieldSource::BinaryOverride
                        } else {
                            FieldSource::Unset
                        },
                        value: o.default_space.clone(),
                    });
                } else {
                    auth_mode = Some(TracedField {
                        value: None,
                        source: FieldSource::Unset,
                    });
                    auth_endpoint = Some(TracedField {
                        value: None,
                        source: FieldSource::Unset,
                    });
                    account_id = Some(TracedField {
                        value: None,
                        source: FieldSource::Unset,
                    });
                    default_instance = Some(TracedField {
                        value: None,
                        source: FieldSource::Unset,
                    });
                    default_space = Some(TracedField {
                        value: None,
                        source: FieldSource::Unset,
                    });
                }
            }
        }

        if matches!(binary, Binary::VeTos) && control_endpoint.value.is_none() {
            if let Some(derived) = derive_tos_control_endpoint(endpoint.value.as_deref()) {
                control_endpoint = TracedField {
                    value: Some(derived),
                    source: FieldSource::Derived,
                };
            }
        }
        if matches!(binary, Binary::Tos) {
            // [Review Fix #1] ByteCloud `tos` has no control plane surface, so
            // keep `control_endpoint` out of the effective view even if a
            // legacy/shared config file still contains that field.
            control_endpoint = TracedField {
                value: None,
                source: FieldSource::Unset,
            };
        } else {
            psm = TracedField {
                value: None,
                source: FieldSource::Unset,
            };
            idc = TracedField {
                value: None,
                source: FieldSource::Unset,
            };
            cluster = TracedField {
                value: None,
                source: FieldSource::Unset,
            };
            addr_family = TracedField {
                value: None,
                source: FieldSource::Unset,
            };
        }

        if psm.value.is_none() {
            // PSM modifiers are meaningful only when PSM itself is configured.
            idc = TracedField {
                value: None,
                source: FieldSource::Unset,
            };
            cluster = TracedField {
                value: None,
                source: FieldSource::Unset,
            };
            addr_family = TracedField {
                value: None,
                source: FieldSource::Unset,
            };
        }

        if matches!(binary, Binary::Tos | Binary::VeTos | Binary::Adrive) {
            fill_builtin_default(&mut checkpoint_dir, DEFAULT_TOS_CHECKPOINT_DIR);
            fill_builtin_default(&mut batch_report_dir, DEFAULT_TOS_BATCH_REPORT_DIR);
            fill_builtin_default(&mut batch_report_format, DEFAULT_TOS_BATCH_REPORT_FORMAT);
            fill_builtin_bool_default(&mut progress_enabled, DEFAULT_TOS_PROGRESS_ENABLED);
            fill_builtin_default(
                &mut checkpoint_threshold,
                DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD,
            );
            fill_builtin_usize_default(&mut batch_concurrency, DEFAULT_BATCH_CONCURRENCY);
            fill_builtin_usize_default(&mut list_concurrency, DEFAULT_LIST_CONCURRENCY);
            fill_builtin_usize_default(&mut multipart_concurrency, DEFAULT_MULTIPART_CONCURRENCY);
            fill_builtin_default(&mut progress_granularity, DEFAULT_PROGRESS_GRANULARITY);
            fill_builtin_default(&mut overwrite_strategy, DEFAULT_OVERWRITE_STRATEGY);
        }
        fill_builtin_u32_default(&mut max_retry_count, DEFAULT_HTTP_MAX_RETRY_COUNT);
        fill_builtin_u64_default(&mut requesttimeout, DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS);
        fill_builtin_u64_default(&mut connecttimeout, DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS);
        fill_builtin_usize_default(&mut maxconnections, DEFAULT_HTTP_MAX_CONNECTIONS);

        // 3) 敏感字段若为 ENC: 密文，尝试解密
        // [Review Fix #1] Use the config file's directory so a custom
        // --config-path decrypts with the same local key used during save.
        try_decrypt_in_dir(&mut access_key_id, config_dir)?;
        try_decrypt_in_dir(&mut secret_access_key, config_dir)?;
        try_decrypt_in_dir(&mut security_token, config_dir)?;

        Ok(EffectiveProfile {
            profile_name: profile_name.to_string(),
            binary: binary.as_str().to_string(),
            region,
            endpoint,
            auth_endpoint,
            psm,
            idc,
            cluster,
            addr_family,
            control_endpoint,
            checkpoint_dir,
            batch_report_dir,
            batch_report_format,
            progress_enabled,
            checkpoint_threshold,
            batch_concurrency,
            list_concurrency,
            multipart_concurrency,
            progress_granularity,
            overwrite_strategy,
            max_retry_count,
            requesttimeout,
            connecttimeout,
            maxconnections,
            access_key_id,
            secret_access_key,
            security_token,
            auth_mode,
            access_token: None,
            refresh_token: None,
            account_id,
            default_instance,
            default_space,
        })
    }

    /// Compute an effective profile without materializing local AK/SK/session credentials.
    ///
    /// Resource, transport, control-plane, and authentication-mode settings
    /// are retained. Credential fields are cleared before the normal resolver
    /// reaches its decryption phase, so this method never reads a local
    /// encryption key.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::ConfigMissing`] when `profile_name` is absent or
    /// isolated to a sibling TOS namespace, and returns validation errors for
    /// invalid settings in the selected effective profile.
    pub fn get_effective_profile_without_credentials_in_dir(
        &self,
        profile_name: &str,
        binary: Binary,
        config_dir: &Path,
    ) -> Result<EffectiveProfile, CliError> {
        let mut non_secret_config = self.clone();
        if let Some(profile) = non_secret_config.profiles.get_mut(profile_name) {
            profile.clear_aksk_credentials();
        }
        non_secret_config.get_effective_profile_in_dir(profile_name, binary, config_dir)
    }
}

/// Merge ByteCloud TOS runtime profiles with endpoint/PSM as mutually exclusive
/// connection modes.
///
/// The layer order is environment, config file, then CLI. A non-empty PSM in
/// the environment or config layer clears endpoint values inherited from lower
/// layers, so `BYTE_TOS_ENDPOINT` cannot accidentally suppress configured PSM.
/// CLI `--endpoint` remains the strongest explicit override.
pub fn merge_tos_runtime_profile(
    env_profile: Profile,
    config_profile: Profile,
    cli_profile: Profile,
) -> Profile {
    let mut effective = Profile::default();
    merge_tos_runtime_layer(&mut effective, env_profile, true);
    merge_tos_runtime_layer(&mut effective, config_profile, true);
    merge_tos_runtime_layer(&mut effective, cli_profile, false);
    effective
}

fn merge_tos_runtime_layer(effective: &mut Profile, mut layer: Profile, prefer_psm: bool) {
    let has_endpoint = has_non_empty_config_value(layer.endpoint.as_deref());
    let has_psm = has_non_empty_config_value(layer.psm.as_deref());

    if has_psm && (prefer_psm || !has_endpoint) {
        // [Review Fix #PSM-EndpointMode-1] PSM is a connection mode, not an
        // independent field; clear lower-precedence endpoints before merging.
        effective.endpoint = None;
        if prefer_psm {
            layer.endpoint = None;
        }
    } else if has_endpoint {
        effective.psm = None;
        effective.idc = None;
        effective.cluster = None;
        effective.addr_family = None;
        if !prefer_psm {
            layer.psm = None;
            layer.idc = None;
            layer.cluster = None;
            layer.addr_family = None;
        }
    }

    *effective = effective.merge(&layer);
}

fn has_non_empty_config_value(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|value| !value.is_empty())
}

fn ensure_tos_namespace_isolated(
    profile_name: &str,
    profile: &Profile,
    binary: Binary,
) -> Result<(), CliError> {
    match binary {
        Binary::Tos if profile.tos.is_none() && profile.ve_tos.is_some() => {
            // [Review Fix #9] Keep the new ByteCloud `tos` profile isolated
            // from the legacy `ve-tos` profile. A sibling override is an
            // explicit signal that this profile is service-specific, so do not
            // silently fall back to shared fields and make both tools appear to
            // use the same configuration.
            Err(CliError::ConfigMissing(format!(
                "Profile '{}' contains [{}.ve-tos] settings but no [{}.tos] settings; `tos` will not consume the `ve-tos` namespace",
                profile_name, profile_name, profile_name
            )))
        }
        Binary::VeTos if profile.ve_tos.is_none() && profile.tos.is_some() => {
            // [Review Fix #9] Symmetric isolation for the legacy `ve-tos`
            // surface after `tos` became the ByteCloud CLI namespace.
            Err(CliError::ConfigMissing(format!(
                "Profile '{}' contains [{}.tos] settings but no [{}.ve-tos] settings; `ve-tos` will not consume the `tos` namespace",
                profile_name, profile_name, profile_name
            )))
        }
        _ => Ok(()),
    }
}

/// Returns true when a profile has only the sibling TOS namespace for `binary`.
///
/// This is used by read-only inspection commands that list many profiles:
/// runtime profile resolution still calls [`ConfigFile::get_effective_profile`]
/// and keeps the strict namespace isolation error.
pub fn has_only_sibling_tos_namespace(profile: &Profile, binary: Binary) -> bool {
    match binary {
        Binary::Tos => profile.tos.is_none() && profile.ve_tos.is_some(),
        Binary::VeTos => profile.ve_tos.is_none() && profile.tos.is_some(),
        _ => false,
    }
}

/// 基于 data endpoint 推导 tos control endpoint。
///
/// 推导规则遵循设计文档 7.3：
/// `tos-xxx` -> `tos-control-xxx`，保留原有 scheme 与 path。
pub fn derive_tos_control_endpoint(data_endpoint: Option<&str>) -> Option<String> {
    let endpoint = data_endpoint?.trim();
    if endpoint.is_empty() {
        return None;
    }

    let (prefix, rest) = match endpoint.find("://") {
        Some(idx) => (&endpoint[..idx + 3], &endpoint[idx + 3..]),
        None => ("", endpoint),
    };
    let (host, suffix) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    let derived_host = host
        .strip_prefix("tos-")
        .map(|tail| format!("tos-control-{}", tail))?;
    Some(format!("{}{}{}", prefix, derived_host, suffix))
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 将敏感字段的明文加密为 ENC:；已经是 ENC: 前缀的不动。
fn encrypt_profile_in_place(profile: &mut Profile, key: &[u8; 32]) -> Result<(), CliError> {
    maybe_encrypt(&mut profile.access_key_id, key)?;
    maybe_encrypt(&mut profile.secret_access_key, key)?;
    maybe_encrypt(&mut profile.security_token, key)?;

    if let Some(o) = profile.tos.as_mut() {
        maybe_encrypt(&mut o.access_key_id, key)?;
        maybe_encrypt(&mut o.secret_access_key, key)?;
        maybe_encrypt(&mut o.security_token, key)?;
    }
    if let Some(o) = profile.ve_tos.as_mut() {
        maybe_encrypt(&mut o.access_key_id, key)?;
        maybe_encrypt(&mut o.secret_access_key, key)?;
        maybe_encrypt(&mut o.security_token, key)?;
    }
    if let Some(o) = profile.tosvector.as_mut() {
        maybe_encrypt(&mut o.access_key_id, key)?;
        maybe_encrypt(&mut o.secret_access_key, key)?;
        maybe_encrypt(&mut o.security_token, key)?;
    }
    if let Some(o) = profile.tostable.as_mut() {
        maybe_encrypt(&mut o.access_key_id, key)?;
        maybe_encrypt(&mut o.secret_access_key, key)?;
        maybe_encrypt(&mut o.security_token, key)?;
    }
    if let Some(o) = profile.adrive.as_mut() {
        maybe_encrypt(&mut o.access_key_id, key)?;
        maybe_encrypt(&mut o.secret_access_key, key)?;
        maybe_encrypt(&mut o.security_token, key)?;
    }
    Ok(())
}

fn maybe_encrypt(field: &mut Option<String>, key: &[u8; 32]) -> Result<(), CliError> {
    if let Some(v) = field.as_ref() {
        if !crypto::is_encrypted(v) && !v.is_empty() && !is_placeholder(v) {
            let enc = crypto::encrypt_with_key(key, v)?;
            *field = Some(enc);
        }
    }
    Ok(())
}

fn is_placeholder(v: &str) -> bool {
    v.starts_with('<') && v.ends_with('>')
}

/// Normalize a credential field read from the config file: treat placeholder
/// tokens (e.g. "<YOUR_ACCESS_KEY_ID>" written by `config init`) and empty
/// strings as unset (`None`).
fn strip_placeholder(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty() && !is_placeholder(v))
}

fn try_decrypt_in_dir(field: &mut TracedField<String>, config_dir: &Path) -> Result<(), CliError> {
    if let Some(v) = field.value.as_ref() {
        if crypto::is_encrypted(v) {
            let pt = crypto::decrypt_in_dir(config_dir, v)?;
            field.value = Some(pt);
        }
    }
    Ok(())
}

fn fill_builtin_default(field: &mut TracedField<String>, value: &str) {
    if field.value.is_none() {
        field.value = Some(value.to_string());
        field.source = FieldSource::Derived;
    }
}

fn fill_builtin_bool_default(field: &mut TracedField<bool>, value: bool) {
    if field.value.is_none() {
        field.value = Some(value);
        field.source = FieldSource::Derived;
    }
}

fn fill_builtin_u32_default(field: &mut TracedField<u32>, value: u32) {
    if field.value.is_none() {
        field.value = Some(value);
        field.source = FieldSource::Derived;
    }
}

fn fill_builtin_u64_default(field: &mut TracedField<u64>, value: u64) {
    if field.value.is_none() {
        field.value = Some(value);
        field.source = FieldSource::Derived;
    }
}

fn fill_builtin_usize_default(field: &mut TracedField<usize>, value: usize) {
    if field.value.is_none() {
        field.value = Some(value);
        field.source = FieldSource::Derived;
    }
}

fn parse_bool(value: &str) -> Result<bool, CliError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(CliError::ValidationError(format!(
            "invalid boolean value '{}': expected true/false",
            value
        ))),
    }
}

fn env_var_any(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| std::env::var(key).ok())
}

fn env_key(prefix: &str, suffix: &str) -> String {
    format!("{prefix}_{suffix}")
}

fn env_var(prefix: &str, suffix: &str) -> Option<String> {
    std::env::var(env_key(prefix, suffix)).ok()
}

fn canonical_config_key(key: &str) -> &str {
    match key {
        "request_timeout" => "requesttimeout",
        "connect_timeout" => "connecttimeout",
        "max_connections" => "maxconnections",
        "addr-family" => "addr_family",
        "checkpoint-threshold" => "checkpoint_threshold",
        "batch-concurrency" => "batch_concurrency",
        "list-concurrency" => "list_concurrency",
        "multipart-concurrency" => "multipart_concurrency",
        "progress-granularity" => "progress_granularity",
        "overwrite-strategy" => "overwrite_strategy",
        other => other,
    }
}

fn parse_u32_field(value: &str, field: &str) -> Result<u32, CliError> {
    value.trim().parse::<u32>().map_err(|_| {
        CliError::ValidationError(format!(
            "invalid value '{}' for {}: expected an unsigned integer",
            value, field
        ))
    })
}

fn parse_positive_u64_field(value: &str, field: &str) -> Result<u64, CliError> {
    let parsed = value.trim().parse::<u64>().map_err(|_| {
        CliError::ValidationError(format!(
            "invalid value '{}' for {}: expected a positive integer",
            value, field
        ))
    })?;
    if parsed == 0 {
        return Err(CliError::ValidationError(format!(
            "invalid value '{}' for {}: expected a positive integer",
            value, field
        )));
    }
    Ok(parsed)
}

fn parse_positive_usize_field(value: &str, field: &str) -> Result<usize, CliError> {
    let parsed = value.trim().parse::<usize>().map_err(|_| {
        CliError::ValidationError(format!(
            "invalid value '{}' for {}: expected a positive integer",
            value, field
        ))
    })?;
    if parsed == 0 {
        return Err(CliError::ValidationError(format!(
            "invalid value '{}' for {}: expected a positive integer",
            value, field
        )));
    }
    Ok(parsed)
}

fn set_shared_field(profile: &mut Profile, key: &str, value: &str) -> Result<(), CliError> {
    match canonical_config_key(key) {
        "region" => profile.region = Some(value.to_string()),
        "access_key_id" => profile.access_key_id = Some(value.to_string()),
        "secret_access_key" => profile.secret_access_key = Some(value.to_string()),
        "security_token" => profile.security_token = Some(value.to_string()),
        "endpoint" => profile.endpoint = Some(value.to_string()),
        "checkpoint_dir" => profile.checkpoint_dir = Some(value.to_string()),
        "batch_report_dir" => profile.batch_report_dir = Some(value.to_string()),
        "batch_report_format" => profile.batch_report_format = Some(value.to_string()),
        "progress_enabled" => profile.progress_enabled = Some(parse_bool(value)?),
        "checkpoint_threshold" => profile.checkpoint_threshold = Some(value.to_string()),
        "batch_concurrency" => {
            profile.batch_concurrency = Some(parse_positive_usize_field(value, key)?)
        }
        "list_concurrency" => {
            profile.list_concurrency = Some(parse_positive_usize_field(value, key)?)
        }
        "multipart_concurrency" => {
            profile.multipart_concurrency = Some(parse_positive_usize_field(value, key)?)
        }
        "progress_granularity" => profile.progress_granularity = Some(value.to_string()),
        "overwrite_strategy" => profile.overwrite_strategy = Some(value.to_string()),
        "max_retry_count" => profile.max_retry_count = Some(parse_u32_field(value, key)?),
        "requesttimeout" => profile.requesttimeout = Some(parse_positive_u64_field(value, key)?),
        "connecttimeout" => profile.connecttimeout = Some(parse_positive_u64_field(value, key)?),
        "maxconnections" => profile.maxconnections = Some(parse_positive_usize_field(value, key)?),
        _ => {
            return Err(CliError::ValidationError(format!(
                "Unknown shared config key '{}'. Valid keys: region, access_key_id, secret_access_key, security_token, endpoint, checkpoint_dir, batch_report_dir, batch_report_format, progress_enabled, checkpoint_threshold, batch_concurrency, list_concurrency, multipart_concurrency, progress_granularity, overwrite_strategy, max_retry_count, requesttimeout, connecttimeout, maxconnections",
                key
            )));
        }
    }
    Ok(())
}

fn set_binary_field(
    profile: &mut Profile,
    binary: &str,
    key: &str,
    value: &str,
) -> Result<(), CliError> {
    let bin = Binary::parse(binary).ok_or_else(|| {
        CliError::ValidationError(format!(
            "Unknown binary '{}'. Valid: tos, ve-tos, tosvector, tostable, adrive",
            binary
        ))
    })?;
    match bin {
        Binary::Tos => {
            let o = profile.tos.get_or_insert_with(TosOverride::default);
            set_tos_override(o, key, value, false)
        }
        Binary::VeTos => {
            let o = profile.ve_tos.get_or_insert_with(TosOverride::default);
            set_tos_override(o, key, value, true)
        }
        Binary::TosVector => {
            let o = profile
                .tosvector
                .get_or_insert_with(TosVectorOverride::default);
            set_tosvector_override(o, key, value)
        }
        Binary::TosTable => {
            let o = profile
                .tostable
                .get_or_insert_with(TosTableOverride::default);
            set_tostable_override(o, key, value)
        }
        Binary::Adrive => {
            let o = profile.adrive.get_or_insert_with(AdriveOverride::default);
            set_adrive_override(o, key, value)
        }
    }
}

fn set_tos_override(
    o: &mut TosOverride,
    key: &str,
    value: &str,
    is_ve_tos: bool,
) -> Result<(), CliError> {
    match canonical_config_key(key) {
        "auth_mode" => o.auth_mode = Some(normalize_tos_auth_mode(value, is_ve_tos)?),
        "region" => o.region = Some(value.into()),
        "endpoint" => o.endpoint = Some(value.into()),
        "psm" => o.psm = Some(value.into()),
        "idc" => o.idc = Some(value.into()),
        "cluster" => o.cluster = Some(value.into()),
        "addr_family" => o.addr_family = Some(value.into()),
        "control_endpoint" => o.control_endpoint = Some(value.into()),
        "account_id" => o.account_id = Some(value.into()),
        "access_key_id" => o.access_key_id = Some(value.into()),
        "secret_access_key" => o.secret_access_key = Some(value.into()),
        "security_token" => o.security_token = Some(value.into()),
        "checkpoint_dir" => o.checkpoint_dir = Some(value.into()),
        "batch_report_dir" => o.batch_report_dir = Some(value.into()),
        "batch_report_format" => o.batch_report_format = Some(value.into()),
        "progress_enabled" => o.progress_enabled = Some(parse_bool(value)?),
        "checkpoint_threshold" => o.checkpoint_threshold = Some(value.into()),
        "batch_concurrency" => o.batch_concurrency = Some(parse_positive_usize_field(value, key)?),
        "list_concurrency" => o.list_concurrency = Some(parse_positive_usize_field(value, key)?),
        "multipart_concurrency" => {
            o.multipart_concurrency = Some(parse_positive_usize_field(value, key)?)
        }
        "progress_granularity" => o.progress_granularity = Some(value.into()),
        "overwrite_strategy" => o.overwrite_strategy = Some(value.into()),
        "max_retry_count" => o.max_retry_count = Some(parse_u32_field(value, key)?),
        "requesttimeout" => o.requesttimeout = Some(parse_positive_u64_field(value, key)?),
        "connecttimeout" => o.connecttimeout = Some(parse_positive_u64_field(value, key)?),
        "maxconnections" => o.maxconnections = Some(parse_positive_usize_field(value, key)?),
        _ => {
            return Err(CliError::ValidationError(format!(
                "Unknown tos override key '{}'",
                key
            )))
        }
    }
    Ok(())
}

fn set_tosvector_override(
    o: &mut TosVectorOverride,
    key: &str,
    value: &str,
) -> Result<(), CliError> {
    match canonical_config_key(key) {
        "region" => o.region = Some(value.into()),
        "endpoint" => o.endpoint = Some(value.into()),
        "access_key_id" => o.access_key_id = Some(value.into()),
        "secret_access_key" => o.secret_access_key = Some(value.into()),
        "security_token" => o.security_token = Some(value.into()),
        "max_retry_count" => o.max_retry_count = Some(parse_u32_field(value, key)?),
        "requesttimeout" => o.requesttimeout = Some(parse_positive_u64_field(value, key)?),
        "connecttimeout" => o.connecttimeout = Some(parse_positive_u64_field(value, key)?),
        "maxconnections" => o.maxconnections = Some(parse_positive_usize_field(value, key)?),
        _ => {
            return Err(CliError::ValidationError(format!(
                "Unknown tosvector override key '{}'",
                key
            )))
        }
    }
    Ok(())
}

fn set_tostable_override(o: &mut TosTableOverride, key: &str, value: &str) -> Result<(), CliError> {
    match canonical_config_key(key) {
        "region" => o.region = Some(value.into()),
        "endpoint" => o.endpoint = Some(value.into()),
        "access_key_id" => o.access_key_id = Some(value.into()),
        "secret_access_key" => o.secret_access_key = Some(value.into()),
        "security_token" => o.security_token = Some(value.into()),
        "max_retry_count" => o.max_retry_count = Some(parse_u32_field(value, key)?),
        "requesttimeout" => o.requesttimeout = Some(parse_positive_u64_field(value, key)?),
        "connecttimeout" => o.connecttimeout = Some(parse_positive_u64_field(value, key)?),
        "maxconnections" => o.maxconnections = Some(parse_positive_usize_field(value, key)?),
        _ => {
            return Err(CliError::ValidationError(format!(
                "Unknown tostable override key '{}'",
                key
            )))
        }
    }
    Ok(())
}

fn set_adrive_override(o: &mut AdriveOverride, key: &str, value: &str) -> Result<(), CliError> {
    match canonical_config_key(key) {
        "auth_mode" => {
            o.auth_mode = Some(normalize_adrive_auth_mode(value)?);
        }
        "region" => o.region = Some(value.into()),
        "endpoint" => o.endpoint = Some(value.into()),
        "auth_endpoint" => o.auth_endpoint = Some(value.into()),
        "access_key_id" => o.access_key_id = Some(value.into()),
        "secret_access_key" => o.secret_access_key = Some(value.into()),
        "security_token" => o.security_token = Some(value.into()),
        "account_id" => o.account_id = Some(value.into()),
        "default_instance" => o.default_instance = Some(value.into()),
        "default_space" => o.default_space = Some(value.into()),
        "checkpoint_dir" => o.checkpoint_dir = Some(value.into()),
        "batch_report_dir" => o.batch_report_dir = Some(value.into()),
        "batch_report_format" => o.batch_report_format = Some(value.into()),
        "progress_enabled" => o.progress_enabled = Some(parse_bool(value)?),
        "checkpoint_threshold" => o.checkpoint_threshold = Some(value.into()),
        "batch_concurrency" => o.batch_concurrency = Some(parse_positive_usize_field(value, key)?),
        "list_concurrency" => o.list_concurrency = Some(parse_positive_usize_field(value, key)?),
        "multipart_concurrency" => {
            o.multipart_concurrency = Some(parse_positive_usize_field(value, key)?)
        }
        "progress_granularity" => o.progress_granularity = Some(value.into()),
        "overwrite_strategy" => o.overwrite_strategy = Some(value.into()),
        "max_retry_count" => o.max_retry_count = Some(parse_u32_field(value, key)?),
        "requesttimeout" => o.requesttimeout = Some(parse_positive_u64_field(value, key)?),
        "connecttimeout" => o.connecttimeout = Some(parse_positive_u64_field(value, key)?),
        "maxconnections" => o.maxconnections = Some(parse_positive_usize_field(value, key)?),
        _ => {
            return Err(CliError::ValidationError(format!(
                "Unknown adrive override key '{}'",
                key
            )))
        }
    }
    Ok(())
}

fn normalize_tos_auth_mode(value: &str, is_ve_tos: bool) -> Result<String, CliError> {
    if is_ve_tos {
        normalize_ve_tos_auth_mode(value)
    } else {
        crate::infra::byte_tos_auth::ByteTosAuthMode::parse(value, "profile config")
            .map(|mode| mode.as_str().to_string())
    }
}

fn normalize_ve_tos_auth_mode(value: &str) -> Result<String, CliError> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "aksk" | "unified") {
        return Ok(normalized);
    }
    Err(CliError::ValidationError(format!(
        "invalid ve-tos auth_mode '{}': expected aksk or unified",
        value
    )))
}

fn normalize_adrive_auth_mode(value: &str) -> Result<String, CliError> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "aksk" | "oauth" | "unified") {
        return Ok(normalized);
    }
    Err(CliError::ValidationError(format!(
        "invalid ADrive auth_mode '{}': expected aksk, oauth, or unified",
        value
    )))
}

/// 将一个密文/明文字符串遮蔽为 `****xxxx` 样式；`ENC:` 前缀会被保留替换为 `ENC:****`。
pub fn mask_secret(s: &str) -> String {
    if s.starts_with(crypto::ENC_PREFIX) {
        "ENC:****".to_string()
    } else if s.len() <= 4 {
        "****".to_string()
    } else {
        format!("****{}", &s[s.len() - 4..])
    }
}

/// 遮蔽 EffectiveProfile 的敏感字段，用于 `tos config show`。
pub fn redact_effective(mut e: EffectiveProfile) -> EffectiveProfile {
    if let Some(v) = e.access_key_id.value.as_ref() {
        e.access_key_id.value = Some(mask_secret(v));
    }
    if let Some(v) = e.secret_access_key.value.as_ref() {
        e.secret_access_key.value = Some(mask_secret(v));
    }
    if let Some(v) = e.security_token.value.as_ref() {
        e.security_token.value = Some(mask_secret(v));
    }
    e
}

/// 判断字段是否属于敏感字段（外部调用）。
pub fn is_sensitive_field(name: &str) -> bool {
    is_secret_field(name)
}

// ---------------------------------------------------------------------------
// 兼容别名：部分旧代码仍然按照 `profile.region` 扁平写入 HashMap<String, Profile>
// ---------------------------------------------------------------------------

impl ConfigFile {
    /// 旧式 `set_value("default", "region", "cn-beijing")` API 兼容。
    /// 等价于 `set_by_path(&[profile_name, key], value)`。
    pub fn set_value(
        &mut self,
        profile_name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), CliError> {
        self.set_by_path(&[profile_name, key], value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static PROFILE_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvironmentRestore {
        values: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvironmentRestore {
        fn capture(names: &[&'static str]) -> Self {
            Self {
                values: names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvironmentRestore {
        fn drop(&mut self) {
            for (name, value) in self.values.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn non_secret_env_profile_retains_resources_and_excludes_credentials() {
        let _lock = PROFILE_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let names = [
            "TOS_REGION",
            "TOS_ENDPOINT",
            "TOS_BATCH_CONCURRENCY",
            "TOS_ACCESS_KEY",
            "TOS_SECRET_KEY",
            "TOS_SECURITY_TOKEN",
        ];
        let _restore = EnvironmentRestore::capture(&names);
        std::env::set_var("TOS_REGION", "cn-beijing");
        std::env::set_var("TOS_ENDPOINT", "https://tos.example.com");
        std::env::set_var("TOS_BATCH_CONCURRENCY", "7");
        std::env::set_var("TOS_ACCESS_KEY", "must-not-be-selected");
        std::env::set_var("TOS_SECRET_KEY", "must-not-be-selected");
        std::env::set_var("TOS_SECURITY_TOKEN", "must-not-be-selected");

        let profile = Profile::from_env_without_credentials();

        assert_eq!(profile.region.as_deref(), Some("cn-beijing"));
        assert_eq!(profile.endpoint.as_deref(), Some("https://tos.example.com"));
        assert_eq!(profile.batch_concurrency, Some(7));
        assert!(profile.access_key_id.is_none());
        assert!(profile.secret_access_key.is_none());
        assert!(profile.security_token.is_none());
    }

    #[test]
    fn byte_tos_non_secret_env_retains_psm_and_network_fields() {
        let _lock = PROFILE_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let settings = [
            ("BYTE_TOS_PSM", "test.psm"),
            ("BYTE_TOS_IDC", "test-idc"),
            ("BYTE_TOS_CLUSTER", "test-cluster"),
            ("BYTE_TOS_ADDR_FAMILY", "ipv6"),
            ("BYTE_TOS_REGION", "cn-test"),
            ("BYTE_TOS_ENDPOINT", "https://test.example"),
            ("BYTE_TOS_ACCESS_KEY", "unused"),
            ("BYTE_TOS_SECRET_KEY", "unused"),
            ("BYTE_TOS_SECURITY_TOKEN", "unused"),
        ];
        let names = settings.iter().map(|(name, _)| *name).collect::<Vec<_>>();
        let _restore = EnvironmentRestore::capture(&names);
        for (name, value) in settings {
            std::env::set_var(name, value);
        }
        let profile = Profile::from_byte_tos_env_without_credentials();
        assert_eq!(profile.psm.as_deref(), Some("test.psm"));
        assert_eq!(profile.idc.as_deref(), Some("test-idc"));
        assert_eq!(profile.cluster.as_deref(), Some("test-cluster"));
        assert_eq!(profile.addr_family.as_deref(), Some("ipv6"));
        assert_eq!(profile.region.as_deref(), Some("cn-test"));
        assert_eq!(profile.endpoint.as_deref(), Some("https://test.example"));
        assert!(profile.access_key_id.is_none());
        assert!(profile.secret_access_key.is_none());
        assert!(profile.security_token.is_none());
        std::env::set_var("BYTE_TOS_PSM", "   ");
        let blank_psm = Profile::from_byte_tos_env_without_credentials();
        assert!(blank_psm.psm.is_none());
        assert!(blank_psm.idc.is_none());
        assert!(blank_psm.cluster.is_none());
        assert!(blank_psm.addr_family.is_none());
    }

    #[test]
    fn no_credentials_effective_profile_retains_resources_without_decrypting() {
        let directory = std::env::temp_dir().join(format!(
            "ve-tos-non-secret-profile-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let mut config = ConfigFile::default();
        let profile = config.get_or_insert_profile("selected");
        profile.region = Some("cn-beijing".to_string());
        profile.access_key_id = Some("ENC:not-valid".to_string());
        profile.secret_access_key = Some("ENC:not-valid".to_string());
        profile.security_token = Some("ENC:not-valid".to_string());
        profile.ve_tos = Some(TosOverride {
            auth_mode: Some("unified".to_string()),
            endpoint: Some("https://tos.example.com".to_string()),
            control_endpoint: Some("https://tos-control.example.com".to_string()),
            max_retry_count: Some(7),
            access_key_id: Some("ENC:not-valid".to_string()),
            secret_access_key: Some("ENC:not-valid".to_string()),
            security_token: Some("ENC:not-valid".to_string()),
            ..TosOverride::default()
        });

        let effective = config
            .get_effective_profile_without_credentials_in_dir("selected", Binary::VeTos, &directory)
            .unwrap();

        assert_eq!(effective.region.value.as_deref(), Some("cn-beijing"));
        assert_eq!(
            effective.endpoint.value.as_deref(),
            Some("https://tos.example.com")
        );
        assert_eq!(
            effective.control_endpoint.value.as_deref(),
            Some("https://tos-control.example.com")
        );
        assert_eq!(effective.max_retry_count.value, Some(7));
        assert_eq!(
            effective.auth_mode.and_then(|field| field.value).as_deref(),
            Some("unified")
        );
        assert!(effective.access_key_id.value.is_none());
        assert!(effective.secret_access_key.value.is_none());
        assert!(effective.security_token.value.is_none());
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn config_parse_error_does_not_echo_secret_source_line() {
        let directory = std::env::temp_dir().join(format!(
            "tos-config-parse-redaction-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(
            &path,
            "[default.adrive]\nsecret_access_key = \"SECRET_MUST_NOT_LEAK\n",
        )
        .unwrap();

        let error = ConfigFile::load_from(&path).err().expect("parse error");

        assert!(!error.to_string().contains("SECRET_MUST_NOT_LEAK"));
        assert!(error.to_string().contains(&path.display().to_string()));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn ve_tos_auth_mode_setter_normalizes_and_populates_effective_profile() {
        let mut config = ConfigFile::default();

        config
            .set_by_path(&["default", "ve-tos", "auth_mode"], "  UNIFIED  ")
            .unwrap();

        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("[default.ve-tos]"));
        assert!(serialized.contains("auth_mode = \"unified\""));
        assert!(!serialized.contains("[default.tos]"));

        let effective = config
            .get_effective_profile("default", Binary::VeTos)
            .unwrap();
        let auth_mode = effective.auth_mode.expect("ve-tos auth mode");
        assert_eq!(auth_mode.value.as_deref(), Some("unified"));
        assert_eq!(auth_mode.source, FieldSource::BinaryOverride);
    }

    #[test]
    fn ve_tos_auth_mode_setter_rejects_other_values() {
        let error = ConfigFile::default()
            .set_by_path(&["default", "ve-tos", "auth_mode"], "oauth")
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Validation error: invalid ve-tos auth_mode 'oauth': expected aksk or unified"
        );
    }

    #[test]
    fn tos_auth_mode_setter_normalizes_and_rejects_foreign_modes() {
        let mut config = ConfigFile::default();
        config
            .set_by_path(&["default", "tos", "auth_mode"], " ZTI ")
            .unwrap();
        let effective = config
            .get_effective_profile("default", Binary::Tos)
            .unwrap();
        let mode = effective.auth_mode.unwrap();
        assert_eq!(mode.value.as_deref(), Some("zti"));
        assert_eq!(mode.source, FieldSource::BinaryOverride);
        for invalid in ["unified", "oauth", ""] {
            assert!(config
                .set_by_path(&["default", "tos", "auth_mode"], invalid)
                .is_err());
        }
    }

    #[test]
    fn invalid_tos_auth_mode_only_blocks_selected_namespace() {
        let directory = std::env::temp_dir().join(format!("tos-config-auth-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(&path, "[other]\nregion = \"cn-test\"\n[default.ve-tos]\nauth_mode = \"aksk\"\n[default.tos]\nauth_mode = \"unified\"\n").unwrap();
        let config = ConfigFile::load_from(&path).unwrap();
        assert!(config
            .get_effective_profile("default", Binary::Tos)
            .is_err());
        assert!(config
            .get_effective_profile("default", Binary::VeTos)
            .is_ok());
        assert!(config.get_effective_profile("other", Binary::Tos).is_ok());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn handwritten_ve_tos_auth_mode_is_normalized_and_validated() {
        let directory = std::env::temp_dir().join(format!(
            "ve-tos-config-auth-mode-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(&path, "[default.ve-tos]\nauth_mode = \"  UNIFIED  \"\n").unwrap();

        let config = ConfigFile::load_from(&path).unwrap();
        let effective = config
            .get_effective_profile("default", Binary::VeTos)
            .unwrap();

        assert_eq!(
            effective.auth_mode.and_then(|field| field.value).as_deref(),
            Some("unified")
        );

        std::fs::write(&path, "[default.ve-tos]\nauth_mode = \"oauth\"\n").unwrap();
        let config = ConfigFile::load_from(&path).unwrap();
        let error = config
            .get_effective_profile("default", Binary::VeTos)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Validation error: invalid ve-tos auth_mode 'oauth': expected aksk or unified"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn handwritten_invalid_adrive_auth_mode_is_rejected() {
        let directory = std::env::temp_dir().join(format!(
            "adrive-config-auth-mode-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(&path, "[default.adrive]\nauth_mode = \"oidc\"\n").unwrap();

        let config = ConfigFile::load_from(&path).unwrap();
        let error = config
            .get_effective_profile("default", Binary::Adrive)
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Validation error: invalid ADrive auth_mode 'oidc': expected aksk, oauth, or unified"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn invalid_unselected_auth_modes_do_not_block_other_profile_resolution() {
        let directory = std::env::temp_dir().join(format!(
            "unselected-config-auth-mode-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(
            &path,
            concat!(
                "[default.tos]\n",
                "endpoint = \"tos.example.com\"\n",
                "[staging.adrive]\n",
                "auth_mode = \"oidc\"\n",
                "[preview.ve-tos]\n",
                "auth_mode = \"oauth\"\n",
            ),
        )
        .unwrap();

        let config = ConfigFile::load_from(&path).unwrap();
        let effective = config
            .get_effective_profile("default", Binary::Tos)
            .unwrap();

        assert_eq!(effective.endpoint.value.as_deref(), Some("tos.example.com"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn tos_and_ve_tos_auth_mode_namespaces_remain_isolated() {
        let mut config = ConfigFile::default();
        config
            .set_by_path(&["default", "tos", "endpoint"], "byte-tos-endpoint")
            .unwrap();
        config
            .set_by_path(&["default", "ve-tos", "endpoint"], "ve-tos-endpoint")
            .unwrap();
        config
            .set_by_path(&["default", "ve-tos", "auth_mode"], "unified")
            .unwrap();

        let tos = config
            .get_effective_profile("default", Binary::Tos)
            .unwrap();
        let ve_tos = config
            .get_effective_profile("default", Binary::VeTos)
            .unwrap();

        assert_eq!(tos.endpoint.value.as_deref(), Some("byte-tos-endpoint"));
        assert!(tos.auth_mode.unwrap().value.is_none());
        assert_eq!(ve_tos.endpoint.value.as_deref(), Some("ve-tos-endpoint"));
        assert_eq!(
            ve_tos.auth_mode.and_then(|field| field.value).as_deref(),
            Some("unified")
        );
    }

    #[test]
    fn adrive_auth_mode_setter_accepts_all_exact_modes_and_rejects_others() {
        for mode in ["aksk", "oauth", "unified"] {
            let mut config = ConfigFile::default();
            config
                .set_by_path(&["default", "adrive", "auth_mode"], mode)
                .unwrap();
            assert_eq!(
                config
                    .get_effective_profile("default", Binary::Adrive)
                    .unwrap()
                    .auth_mode
                    .and_then(|field| field.value)
                    .as_deref(),
                Some(mode)
            );
        }

        let error = ConfigFile::default()
            .set_by_path(&["default", "adrive", "auth_mode"], "oidc")
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Validation error: invalid ADrive auth_mode 'oidc': expected aksk, oauth, or unified"
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_save_replaces_file_atomically() {
        use std::os::unix::fs::MetadataExt;

        let directory = std::env::temp_dir().join(format!(
            "tos-config-atomic-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        let path = directory.join("config.toml");
        let mut config = ConfigFile::default();
        config
            .set_by_path(&["default", "region"], "cn-beijing")
            .unwrap();
        config.save_to_path(&path).unwrap();
        let first_inode = std::fs::metadata(&path).unwrap().ino();

        config
            .set_by_path(&["default", "region"], "cn-shanghai")
            .unwrap();
        config.save_to_path(&path).unwrap();
        let second_inode = std::fs::metadata(&path).unwrap().ino();

        assert_ne!(first_inode, second_inode);
        assert_eq!(
            ConfigFile::load_from(&path)
                .unwrap()
                .get_profile("default")
                .unwrap()
                .region
                .as_deref(),
            Some("cn-shanghai")
        );
        let has_temporary_file = std::fs::read_dir(&directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("config.tmp-")
        });
        assert!(!has_temporary_file);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn effective_shared_only() {
        let mut cf = ConfigFile::default();
        let p = cf.get_or_insert_profile("default");
        p.region = Some("cn-beijing".into());
        p.endpoint = Some("tos-cn-beijing.volces.com".into());

        let eff = cf.get_effective_profile("default", Binary::Tos).unwrap();
        assert_eq!(eff.region.value.as_deref(), Some("cn-beijing"));
        assert_eq!(eff.region.source, FieldSource::Shared);
        assert_eq!(eff.endpoint.source, FieldSource::Shared);
        assert_eq!(eff.control_endpoint.value.as_deref(), None);
        assert_eq!(eff.control_endpoint.source, FieldSource::Unset);

        let eff = cf.get_effective_profile("default", Binary::VeTos).unwrap();
        assert_eq!(
            eff.control_endpoint.value.as_deref(),
            Some("tos-control-cn-beijing.volces.com")
        );
        assert_eq!(eff.control_endpoint.source, FieldSource::Derived);
        let auth_mode = eff.auth_mode.expect("ve-tos auth mode field");
        assert_eq!(auth_mode.value, None);
        assert_eq!(auth_mode.source, FieldSource::Unset);
    }

    #[test]
    fn effective_binary_override() {
        let mut cf = ConfigFile::default();
        let p = cf.get_or_insert_profile("default");
        p.region = Some("cn-beijing".into());
        p.endpoint = Some("shared-endpoint".into());
        p.ve_tos = Some(TosOverride {
            endpoint: Some("tos-endpoint".into()),
            control_endpoint: Some("tos-control-endpoint".into()),
            ..Default::default()
        });

        let eff = cf.get_effective_profile("default", Binary::VeTos).unwrap();
        assert_eq!(eff.region.value.as_deref(), Some("cn-beijing"));
        assert_eq!(eff.region.source, FieldSource::Shared);
        assert_eq!(eff.endpoint.value.as_deref(), Some("tos-endpoint"));
        assert_eq!(eff.endpoint.source, FieldSource::BinaryOverride);
        assert_eq!(
            eff.control_endpoint.value.as_deref(),
            Some("tos-control-endpoint")
        );
        assert_eq!(eff.control_endpoint.source, FieldSource::BinaryOverride);
    }

    #[test]
    fn merge_tos_runtime_profile_prefers_env_psm_over_env_endpoint() {
        let env_profile = Profile {
            endpoint: Some("tos-cn-north-boe.byted.org".into()),
            psm: Some("toutiao.tos.tosapi".into()),
            addr_family: Some("dual-stack".into()),
            ..Default::default()
        };

        let effective =
            merge_tos_runtime_profile(env_profile, Profile::default(), Profile::default());

        assert_eq!(effective.endpoint, None);
        assert_eq!(effective.psm.as_deref(), Some("toutiao.tos.tosapi"));
        assert_eq!(effective.addr_family.as_deref(), Some("dual-stack"));
    }

    #[test]
    fn merge_tos_runtime_profile_config_psm_clears_env_endpoint() {
        let env_profile = Profile {
            endpoint: Some("tos-cn-north-boe.byted.org".into()),
            ..Default::default()
        };
        let config_profile = Profile {
            psm: Some("toutiao.tos.tosapi".into()),
            ..Default::default()
        };

        let effective = merge_tos_runtime_profile(env_profile, config_profile, Profile::default());

        assert_eq!(effective.endpoint, None);
        assert_eq!(effective.psm.as_deref(), Some("toutiao.tos.tosapi"));
    }

    #[test]
    fn merge_tos_runtime_profile_config_endpoint_clears_env_psm() {
        let env_profile = Profile {
            psm: Some("toutiao.tos.tosapi".into()),
            ..Default::default()
        };
        let config_profile = Profile {
            endpoint: Some("tos-cn-north-boe.byted.org".into()),
            ..Default::default()
        };

        let effective = merge_tos_runtime_profile(env_profile, config_profile, Profile::default());

        assert_eq!(
            effective.endpoint.as_deref(),
            Some("tos-cn-north-boe.byted.org")
        );
        assert_eq!(effective.psm, None);
    }

    #[test]
    fn merge_tos_runtime_profile_cli_endpoint_stays_strongest_mode() {
        let env_profile = Profile {
            psm: Some("toutiao.tos.tosapi".into()),
            ..Default::default()
        };
        let cli_profile = Profile {
            endpoint: Some("tos-cn-north-boe.byted.org".into()),
            psm: Some("ignored.cli.psm".into()),
            ..Default::default()
        };

        let effective = merge_tos_runtime_profile(env_profile, Profile::default(), cli_profile);

        assert_eq!(
            effective.endpoint.as_deref(),
            Some("tos-cn-north-boe.byted.org")
        );
        assert_eq!(effective.psm, None);
    }

    #[test]
    fn set_by_path_all_shapes() {
        let mut cf = ConfigFile::default();
        cf.set_by_path(&["region"], "cn-beijing").unwrap();
        cf.set_by_path(&["default", "endpoint"], "x").unwrap();
        cf.set_by_path(&["default", "tos", "endpoint"], "y")
            .unwrap();
        cf.set_by_path(&["default", "tos", "control_endpoint"], "z")
            .unwrap();
        cf.set_by_path(&["default", "adrive", "account_id"], "2100")
            .unwrap();

        let p = cf.get_profile("default").unwrap();
        assert_eq!(p.region.as_deref(), Some("cn-beijing"));
        assert_eq!(p.endpoint.as_deref(), Some("x"));
        assert_eq!(p.tos.as_ref().unwrap().endpoint.as_deref(), Some("y"));
        assert_eq!(
            p.tos.as_ref().unwrap().control_endpoint.as_deref(),
            Some("z")
        );
        assert_eq!(
            p.adrive.as_ref().unwrap().account_id.as_deref(),
            Some("2100")
        );
    }

    #[test]
    fn adrive_auth_endpoint_is_binary_specific() {
        let mut config = ConfigFile::default();
        config
            .set_by_path(
                &["default", "adrive", "auth_endpoint"],
                "https://idsauth.volces.com",
            )
            .unwrap();

        let adrive = config
            .get_effective_profile("default", Binary::Adrive)
            .unwrap();
        assert_eq!(
            adrive
                .auth_endpoint
                .and_then(|field| field.value)
                .as_deref(),
            Some("https://idsauth.volces.com")
        );
        let tos = config
            .get_effective_profile("default", Binary::Tos)
            .unwrap();
        assert!(tos.auth_endpoint.is_none());
    }

    #[test]
    fn derive_control_endpoint_keeps_scheme_and_path() {
        assert_eq!(
            derive_tos_control_endpoint(Some("https://tos-cn-beijing.volces.com/api")),
            Some("https://tos-control-cn-beijing.volces.com/api".to_string())
        );
        assert_eq!(
            derive_tos_control_endpoint(Some("tos-cn-beijing.volces.com")),
            Some("tos-control-cn-beijing.volces.com".to_string())
        );
        assert_eq!(
            derive_tos_control_endpoint(Some("private.example.com")),
            None
        );
    }

    #[test]
    fn mask_secret_works() {
        assert_eq!(mask_secret("AKTPxxxxxxxx1234"), "****1234");
        assert_eq!(mask_secret("abc"), "****");
        assert_eq!(mask_secret("ENC:abcdefghij"), "ENC:****");
    }

    #[test]
    fn oauth_credential_fields_are_sensitive() {
        for field in [
            "access_token",
            "refresh_token",
            "oauth_client_secret",
            "device_code",
        ] {
            assert!(is_sensitive_field(field), "field={field}");
        }
    }
}
