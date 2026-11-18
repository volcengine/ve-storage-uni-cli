//! Validated platform metadata; no SDK globals, shell probes, or network I/O.
use super::{is_namespace, ExpectedEnvironment};
use crate::platform::TccError;
use reqwest::header::{HeaderMap, HeaderValue};
use std::{collections::BTreeMap, net::IpAddr};
use tos_core::infra::discovery::ConsulLookupRequest;

const PLATFORM_KEYS: &[&str] = &[
    "LOAD_SERVICE_PSM",
    "PSM",
    "TCE_PSM",
    "SERVICE_ENV",
    "TCE_ENV",
    "SERVICE_CLUSTER",
    "TCE_CLUSTER",
    "RUNTIME_IDC_NAME",
    "TCE_HOST_ENV",
    "TESTING_PREFIX",
    "IS_PROD_RUNTIME",
    "TCE_STAGE",
    "MY_POD_NAME",
    "MY_HOST_IP",
    "MY_HOST_IPV6",
];
const MAX_METADATA_BYTES: usize = 256;

pub(crate) struct RuntimeContext {
    pub(super) headers: HeaderMap,
    proxy_psm: &'static str,
    idc: String,
}
impl RuntimeContext {
    pub(crate) fn from_env(expected: ExpectedEnvironment) -> Result<Self, TccError> {
        let mut values = BTreeMap::new();
        for key in PLATFORM_KEYS {
            match std::env::var(key) {
                Ok(value) => {
                    values.insert((*key).into(), value);
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(_) => return Err(TccError::Environment),
            }
        }
        Self::from_map(&values, expected)
    }
    pub(crate) fn from_map(
        values: &BTreeMap<String, String>,
        expected: ExpectedEnvironment,
    ) -> Result<Self, TccError> {
        let psm = first(values, &["LOAD_SERVICE_PSM", "PSM", "TCE_PSM"]).unwrap_or("");
        if !is_namespace(psm) || !psm.contains('.') {
            return Err(TccError::Identity);
        }
        let idc = values
            .get("RUNTIME_IDC_NAME")
            .ok_or(TccError::Environment)?;
        validate_classification(values, expected, idc)?;
        let environment = first(values, &["SERVICE_ENV", "TCE_ENV"]).unwrap_or("prod");
        let cluster = first(values, &["SERVICE_CLUSTER", "TCE_CLUSTER"]).unwrap_or("default");
        if cluster.is_empty() {
            return Err(TccError::Environment);
        }
        let mut headers = HeaderMap::new();
        headers.insert("x-psm", metadata_header(psm)?);
        headers.insert("x-env", metadata_header(environment)?);
        headers.insert("x-cluster", metadata_header(cluster)?);
        add_optional_headers(&mut headers, values)?;
        headers.insert(
            "x-sdk-version",
            HeaderValue::from_static(concat!("ask-auth/", env!("CARGO_PKG_VERSION"))),
        );
        let proxy_psm = match environment {
            "" | "prod" | "canary" => "bytedance.bconfig.proxy",
            _ => "bytedance.bconfig.envs_proxy",
        };
        Ok(Self {
            headers,
            proxy_psm,
            idc: idc.clone(),
        })
    }
    pub(crate) fn headers(&self) -> HeaderMap {
        self.headers.clone()
    }
    pub(crate) fn discovery_request(&self) -> ConsulLookupRequest {
        // [Review Fix #94] Sender cluster metadata must not change the SDK's default target cluster.
        ConsulLookupRequest {
            psm: self.proxy_psm.into(),
            idc: Some(self.idc.clone()),
            cluster: Some("default".into()),
            addr_family: None,
        }
    }
}
fn first<'a>(values: &'a BTreeMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| values.get(*key).map(String::as_str))
}
fn validate_classification(
    values: &BTreeMap<String, String>,
    expected: ExpectedEnvironment,
    idc: &str,
) -> Result<(), TccError> {
    // [Review Fix #95] Require explicit IDC evidence rather than SDK shell fallbacks or unknown identities.
    if idc.is_empty()
        || idc.len() > 64
        || matches!(idc, "unknown" | "-")
        || !idc
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(TccError::Environment);
    }
    let is_boe = matches!(idc, "boe" | "boe2" | "boei18n");
    let is_restricted = values
        .get("TCE_HOST_ENV")
        .is_some_and(|value| value == "ppe")
        || values
            .get("TESTING_PREFIX")
            .is_some_and(|value| value == "offline");
    let is_production =
        !is_boe && (values.contains_key("IS_PROD_RUNTIME") || values.contains_key("SERVICE_ENV"));
    let is_expected = match expected {
        ExpectedEnvironment::Boe => is_boe,
        ExpectedEnvironment::Production => is_production,
    };
    if !is_expected || is_restricted {
        return Err(TccError::Environment);
    }
    Ok(())
}
fn metadata_header(value: &str) -> Result<HeaderValue, TccError> {
    if value.len() > MAX_METADATA_BYTES || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(TccError::Environment);
    }
    HeaderValue::from_str(value).map_err(|_| TccError::Environment)
}
fn add_optional_headers(
    headers: &mut HeaderMap,
    values: &BTreeMap<String, String>,
) -> Result<(), TccError> {
    for (key, header) in [("TCE_STAGE", "x-stage"), ("MY_POD_NAME", "x-pod")] {
        if let Some(value) = values.get(key) {
            headers.insert(header, metadata_header(value)?);
        }
    }
    let mut host_ip = None;
    for key in ["MY_HOST_IP", "MY_HOST_IPV6"] {
        if let Some(value) = values.get(key) {
            let address = value.parse::<IpAddr>().map_err(|_| TccError::Environment)?;
            if address.is_unspecified() || address.is_multicast() {
                return Err(TccError::Environment);
            }
            host_ip = Some(metadata_header(value)?);
        }
    }
    if let Some(value) = host_ip {
        headers.insert("x-host-ip", value);
    }
    Ok(())
}
#[cfg(test)]
mod tests;
