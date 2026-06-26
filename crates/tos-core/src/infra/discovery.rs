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

//! ByteTOS PSM service discovery.
//!
//! This module intentionally implements the small subset of `byted-sd` needed
//! by `tos-cli`: local Consul lookup, BNS bucket routing, and weighted endpoint
//! selection. `ve-tos` and `ve-adrive` never construct these types.

use crate::agent::error::CliError;
use crate::infra::config::Profile;
use rand::distributions::WeightedIndex;
use rand::prelude::Distribution;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BNS_ENDPOINT_TEMPLATE: &str = "http://{}/bns/GetPSMInfo";
const BNS_SERVICE_NAME: &str = "tos.access.bns";
const DEFAULT_CONSUL_HOST: &str = "127.0.0.1";
const DEFAULT_CONSUL_PORT: u16 = 2280;
const DEFAULT_CONSUL_TIMEOUT_MS: u64 = 500;
const DEFAULT_ENDPOINT_WEIGHT: u32 = 10;
const DEFAULT_LOCAL_PSM_WEIGHT: f32 = 100.0;
const DEFAULT_CLUSTER: &str = "default";
const HEADER_REMOTE_PSM: &str = "x-tos-remote-psm";
const TEST_TOSAPI_ADDR_ENV: &str = "TEST_TOSAPI_ADDR";
const TOSV_BOE_ENDPOINT: &str = "http://tosv.boe.byted.org/obj/tos-bns-service/{idc}/{bucket}";
const TOSV_ENDPOINT: &str = "http://tosv.byted.org/obj/tos-bns-service/{idc}/{bucket}";
const TOSV_GISO_ENDPOINT: &str = "http://tosv.byted.org/obj/tos-bns-service-aiso/{idc}/{bucket}";
const TOSV_IBOE_ENDPOINT: &str = "http://tosv.boei18n.byted.org/obj/tos-bns-service/{idc}/{bucket}";
const TOSV_SG_ENDPOINT: &str = "http://tosv.byted.org/obj/tos-bns-service-sg/{idc}/{bucket}";
const TOSV_TTP2_ENDPOINT: &str =
    "http://tosv-ttp2.tiktok-usts.org/obj/tos-bns-service-ttp-tx2/{idc}/{bucket}";
const TOSV_TTP_ENDPOINT: &str =
    "http://tosv.tiktok-usts.org/obj/tos-bns-service-ttp-tx/{idc}/{bucket}";

/// Configuration for resolving a ByteTOS PSM to concrete service addresses.
///
/// `psm` is required. `idc`, `cluster`, and `addr_family` are optional and are
/// only effective when `psm` is present in the runtime profile.
#[derive(Debug, Clone)]
pub struct PsmDiscoveryConfig {
    /// Default service PSM used when BNS has no bucket-specific override.
    pub psm: String,
    /// IDC used for BNS metadata lookup and Consul service lookup.
    pub idc: String,
    /// Cluster filter passed to Consul service lookup.
    pub cluster: String,
    /// Optional Consul address family query value.
    pub addr_family: Option<AddrFamily>,
}

impl PsmDiscoveryConfig {
    /// Build discovery config from a runtime profile.
    ///
    /// Returns `Ok(None)` when the profile has no non-empty PSM. Returns a
    /// validation error if `addr_family` is not one of `v4`, `v6`, or
    /// `dual-stack`.
    pub fn from_profile(profile: &Profile) -> Result<Option<Self>, CliError> {
        let Some(psm) = non_empty_string(profile.psm.as_deref()) else {
            return Ok(None);
        };
        let idc = non_empty_string(profile.idc.as_deref()).unwrap_or_default();
        let cluster =
            non_empty_string(profile.cluster.as_deref()).unwrap_or_else(|| DEFAULT_CLUSTER.into());
        let addr_family = profile
            .addr_family
            .as_deref()
            .and_then(|value| non_empty_string(Some(value)))
            .map(|value| AddrFamily::parse(&value))
            .transpose()?;
        Ok(Some(Self {
            psm,
            idc,
            cluster,
            addr_family,
        }))
    }
}

/// Consul address family query value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrFamily {
    /// Ask Consul for dual-stack endpoints.
    DualStack,
    /// Ask Consul for IPv4 endpoints.
    V4,
    /// Ask Consul for IPv6 endpoints.
    V6,
}

impl AddrFamily {
    fn parse(value: &str) -> Result<Self, CliError> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "dual-stack" => Ok(Self::DualStack),
            "v4" => Ok(Self::V4),
            "v6" => Ok(Self::V6),
            _ => Err(CliError::ValidationError(format!(
                "Invalid addr_family '{}'; expected v4, v6, or dual-stack",
                value
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::DualStack => "dual-stack",
            Self::V4 => "v4",
            Self::V6 => "v6",
        }
    }
}

/// A resolved endpoint with its Consul weight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedEndpoint {
    /// Concrete socket address returned by Consul or static test config.
    pub addr: SocketAddr,
    /// Weight used for client-side load balancing.
    pub weight: u32,
}

impl WeightedEndpoint {
    fn new(addr: SocketAddr, weight: u32) -> Self {
        Self { addr, weight }
    }
}

/// Bucket-aware PSM resolver used by ByteTOS V1 clients.
///
/// The resolver keeps one address manager per bucket because BNS can map
/// different buckets to different backing PSMs.
#[derive(Debug)]
pub struct PsmResolver {
    config: PsmDiscoveryConfig,
    consul: Arc<ConsulLookupClient>,
    managers: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<AddrManager>>>>,
    static_endpoints: Option<Vec<WeightedEndpoint>>,
}

impl PsmResolver {
    /// Create a resolver from PSM discovery config.
    ///
    /// If `TEST_TOSAPI_ADDR` contains at least one valid socket address, that
    /// static list is used instead of Consul/BNS. This mirrors the Rust SDK and
    /// keeps local integration tests deterministic.
    pub fn new(config: PsmDiscoveryConfig) -> Result<Self, CliError> {
        Ok(Self {
            config,
            consul: Arc::new(ConsulLookupClient::from_env()?),
            managers: tokio::sync::Mutex::new(HashMap::new()),
            static_endpoints: static_endpoints_from_env(),
        })
    }

    /// Resolve a bucket to a concrete socket address.
    ///
    /// Returns a transfer error when no usable endpoint is available.
    pub async fn resolve_addr(&self, bucket: &str) -> Result<SocketAddr, CliError> {
        if let Some(static_endpoints) = &self.static_endpoints {
            return choose_weighted_addr(static_endpoints).ok_or_else(empty_endpoint_error);
        }

        let manager = self.manager_for_bucket(bucket).await?;
        let result = manager.lock().await.get_addr().await;
        result
    }

    /// Mark a selected address as successful for future load balancing.
    pub async fn mark_success(&self, bucket: &str, addr: SocketAddr) {
        if let Some(manager) = self.get_existing_manager(bucket).await {
            manager.lock().await.mark_success(addr);
        }
    }

    /// Mark a selected address as failed and remove it from the hot cache.
    pub async fn mark_failure(&self, bucket: &str, addr: SocketAddr) {
        if let Some(manager) = self.get_existing_manager(bucket).await {
            manager.lock().await.mark_failure(addr);
        }
    }

    async fn manager_for_bucket(
        &self,
        bucket: &str,
    ) -> Result<Arc<tokio::sync::Mutex<AddrManager>>, CliError> {
        let mut managers = self.managers.lock().await;
        if let Some(manager) = managers.get(bucket) {
            return Ok(Arc::clone(manager));
        }
        let manager = Arc::new(tokio::sync::Mutex::new(AddrManager::new(
            ServiceResolver::Bns(Arc::new(BnsTask::new(
                bucket,
                self.config.clone(),
                Arc::clone(&self.consul),
            )?)),
        )));
        managers.insert(bucket.to_string(), Arc::clone(&manager));
        Ok(manager)
    }

    async fn get_existing_manager(
        &self,
        bucket: &str,
    ) -> Option<Arc<tokio::sync::Mutex<AddrManager>>> {
        self.managers.lock().await.get(bucket).cloned()
    }
}

#[derive(Debug)]
enum ServiceResolver {
    Bns(Arc<BnsTask>),
}

#[derive(Debug)]
struct AddrManager {
    resolver: ServiceResolver,
    endpoints: Vec<WeightedEndpoint>,
    last_refresh: Option<Instant>,
    refresh_interval: Duration,
}

impl AddrManager {
    fn new(resolver: ServiceResolver) -> Self {
        Self {
            resolver,
            endpoints: Vec::new(),
            last_refresh: None,
            refresh_interval: Duration::from_secs(10),
        }
    }

    async fn get_addr(&mut self) -> Result<SocketAddr, CliError> {
        if self.should_refresh() {
            self.refresh().await?;
        }
        choose_weighted_addr(&self.endpoints).ok_or_else(empty_endpoint_error)
    }

    fn mark_success(&mut self, _addr: SocketAddr) {}

    fn mark_failure(&mut self, addr: SocketAddr) {
        if self.endpoints.len() > 1 {
            self.endpoints.retain(|endpoint| endpoint.addr != addr);
        }
    }

    async fn refresh(&mut self) -> Result<(), CliError> {
        self.endpoints = match &self.resolver {
            ServiceResolver::Bns(task) => task.resolve_endpoints().await?,
        };
        self.last_refresh = Some(Instant::now());
        Ok(())
    }

    fn should_refresh(&self) -> bool {
        self.endpoints.is_empty()
            || self
                .last_refresh
                .map(|last| last.elapsed() >= self.refresh_interval)
                .unwrap_or(true)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PsmEntry {
    #[serde(default, rename = "psm")]
    psm: String,
    #[serde(default)]
    weight: f32,
    #[serde(default)]
    idc: String,
    #[serde(default)]
    cluster: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PsmInfo {
    #[serde(default)]
    psm: Vec<PsmEntry>,
    #[serde(default, rename = "endpoint_weight")]
    _endpoint_weight: f32,
    #[serde(default, rename = "update_time")]
    update_time: i64,
}

#[derive(Debug, Deserialize, Default)]
struct PsmResult {
    #[serde(rename = "psm_info")]
    psm_info: Option<PsmInfo>,
}

#[derive(Debug, Default)]
struct PsmAllocator {
    allocated: bool,
    psm: Vec<PsmEntry>,
    psm_update_ts: i64,
    last_index: usize,
}

impl PsmAllocator {
    fn set_psm_info(&mut self, info: PsmInfo) {
        if info.update_time <= self.psm_update_ts {
            return;
        }
        self.psm = info.psm;
        self.psm_update_ts = info.update_time;
        self.allocated = false;
        self.last_index = 0;
    }

    fn allocate(&mut self, local: &PsmEntry) -> PsmEntry {
        if self.psm.is_empty() {
            return local.clone();
        }
        if self.allocated && self.last_index < self.psm.len() {
            return fill_blank_psm_entry(self.psm[self.last_index].clone(), local);
        }
        self.allocated = true;
        self.allocate_uncached(local)
    }

    fn allocate_uncached(&mut self, local: &PsmEntry) -> PsmEntry {
        let Some(index) = choose_psm_index(&self.psm) else {
            return local.clone();
        };
        self.last_index = index;
        fill_blank_psm_entry(self.psm[index].clone(), local)
    }
}

#[derive(Debug)]
struct BnsTask {
    bucket: String,
    config: PsmDiscoveryConfig,
    consul: Arc<ConsulLookupClient>,
    http: Client,
    allocator: Mutex<PsmAllocator>,
    initialized: AtomicBool,
    last_refresh: Mutex<Instant>,
}

impl BnsTask {
    fn new(
        bucket: impl Into<String>,
        config: PsmDiscoveryConfig,
        consul: Arc<ConsulLookupClient>,
    ) -> Result<Self, CliError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .map_err(CliError::Http)?;
        Ok(Self {
            bucket: bucket.into(),
            config,
            consul,
            http,
            allocator: Mutex::new(PsmAllocator::default()),
            initialized: AtomicBool::new(false),
            last_refresh: Mutex::new(Instant::now()),
        })
    }

    async fn resolve_endpoints(&self) -> Result<Vec<WeightedEndpoint>, CliError> {
        let entry = self.allocate().await;
        self.consul
            .lookup(ConsulLookupRequest {
                psm: if entry.psm.is_empty() {
                    self.config.psm.clone()
                } else {
                    entry.psm
                },
                idc: non_empty_string(Some(&entry.idc)),
                cluster: non_empty_string(Some(&entry.cluster)),
                addr_family: self.config.addr_family,
            })
            .await
    }

    async fn allocate(&self) -> PsmEntry {
        if !self.initialized.swap(true, Ordering::SeqCst) {
            let _ = self.refresh_once().await;
        }
        if self.should_refresh() {
            let _ = self.refresh_once().await;
        }
        let local = PsmEntry {
            psm: self.config.psm.clone(),
            idc: self.config.idc.clone(),
            cluster: self.config.cluster.clone(),
            weight: DEFAULT_LOCAL_PSM_WEIGHT,
        };
        self.allocator
            .lock()
            .map(|mut allocator| allocator.allocate(&local))
            .unwrap_or(local)
    }

    async fn refresh_once(&self) -> Result<(), CliError> {
        if let Some(info) = self.get_psm_info_from_bns().await? {
            self.set_psm_info(info);
            return Ok(());
        }
        if let Some(info) = self.get_psm_info_from_tosv().await? {
            self.set_psm_info(info);
        }
        Ok(())
    }

    async fn get_psm_info_from_bns(&self) -> Result<Option<PsmInfo>, CliError> {
        let addr = self.lookup_bns_endpoint().await?;
        let url = BNS_ENDPOINT_TEMPLATE.replace("{}", &addr.to_string());
        self.get_psm_info(self.http.get(url).query(&[
            ("bucket", self.bucket.as_str()),
            ("idc", self.config.idc.as_str()),
        ]))
        .await
    }

    async fn get_psm_info_from_tosv(&self) -> Result<Option<PsmInfo>, CliError> {
        let url = choose_tosv_endpoint(&self.config.idc)
            .replace("{idc}", &self.config.idc)
            .replace("{bucket}", &self.bucket);
        self.get_psm_info(self.http.get(url)).await
    }

    async fn get_psm_info(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<Option<PsmInfo>, CliError> {
        let resp = request.header(HEADER_REMOTE_PSM, "-").send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(CliError::TransferFailed(format!(
                "PSM lookup returned status {}",
                resp.status()
            )));
        }
        decode_psm_info(&resp.bytes().await?)
    }

    async fn lookup_bns_endpoint(&self) -> Result<SocketAddr, CliError> {
        let endpoints = self
            .consul
            .lookup(ConsulLookupRequest {
                psm: BNS_SERVICE_NAME.to_string(),
                idc: non_empty_string(Some(&self.config.idc)),
                cluster: None,
                // [Review Fix #4] Match tos-rust-sdk: explicit addr_family is
                // used for the final TOS PSM lookup, not the BNS meta-service.
                addr_family: None,
            })
            .await?;
        choose_weighted_addr(&endpoints).ok_or_else(empty_endpoint_error)
    }

    fn set_psm_info(&self, info: PsmInfo) {
        if let Ok(mut allocator) = self.allocator.lock() {
            allocator.set_psm_info(info);
        }
    }

    fn should_refresh(&self) -> bool {
        let Ok(mut last_refresh) = self.last_refresh.lock() else {
            return false;
        };
        if last_refresh.elapsed() <= Duration::from_secs(30) {
            return false;
        }
        *last_refresh = Instant::now();
        true
    }
}

#[derive(Debug, Clone)]
struct ConsulLookupRequest {
    psm: String,
    idc: Option<String>,
    cluster: Option<String>,
    addr_family: Option<AddrFamily>,
}

#[derive(Debug)]
struct ConsulLookupClient {
    http: Client,
    base_url: String,
}

impl ConsulLookupClient {
    fn from_env() -> Result<Self, CliError> {
        let timeout = Duration::from_millis(consul_timeout_ms());
        Ok(Self {
            http: Client::builder()
                .timeout(timeout)
                .build()
                .map_err(CliError::Http)?,
            base_url: format!("http://{}/v1/lookup/name", consul_addr_from_env()),
        })
    }

    async fn lookup(
        &self,
        request: ConsulLookupRequest,
    ) -> Result<Vec<WeightedEndpoint>, CliError> {
        let url = self.lookup_url(&request)?;
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(CliError::TransferFailed(format!(
                "Consul lookup returned status {}",
                resp.status()
            )));
        }
        resp.json::<Vec<ConsulEndpointData>>()
            .await?
            .into_iter()
            .map(ConsulEndpointData::into_weighted_endpoint)
            .collect()
    }

    fn lookup_url(&self, request: &ConsulLookupRequest) -> Result<url::Url, CliError> {
        let mut url = url::Url::parse(&self.base_url)
            .map_err(|err| CliError::ValidationError(format!("Invalid Consul URL: {err}")))?;
        let family_options = AddrFamilyOptions::from_request(request.addr_family);
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("name", &consul_service_name(request));
            if let Some(addr_family) = family_options.addr_family {
                query.append_pair("addr-family", addr_family.as_str());
            }
            if let Some(unique) = family_options.unique {
                query.append_pair("unique", unique.as_str());
            }
            if let Some(cluster) = request.cluster.as_deref() {
                query.append_pair("cluster", cluster);
            }
        }
        Ok(url)
    }
}

#[derive(Debug, Deserialize)]
struct ConsulEndpointData {
    #[serde(rename = "Host")]
    host: String,
    #[serde(rename = "Port")]
    port: u16,
    #[serde(default, rename = "Tags")]
    tags: HashMap<String, String>,
}

impl ConsulEndpointData {
    fn into_weighted_endpoint(self) -> Result<WeightedEndpoint, CliError> {
        let ip = IpAddr::from_str(&self.host).map_err(|err| {
            CliError::TransferFailed(format!("Consul endpoint host is not an IP address: {err}"))
        })?;
        let weight = self
            .tags
            .get("weight")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(DEFAULT_ENDPOINT_WEIGHT);
        Ok(WeightedEndpoint::new(
            SocketAddr::new(ip, self.port),
            weight,
        ))
    }
}

#[derive(Debug, Clone, Copy)]
struct AddrFamilyOptions {
    addr_family: Option<AddrFamily>,
    unique: Option<AddrFamily>,
}

impl AddrFamilyOptions {
    fn from_request(addr_family: Option<AddrFamily>) -> Self {
        let inferred = infer_addr_family_from_env();
        Self {
            addr_family: addr_family.or(inferred.addr_family),
            unique: inferred.unique,
        }
    }
}

fn choose_weighted_addr(endpoints: &[WeightedEndpoint]) -> Option<SocketAddr> {
    choose_weighted_index(endpoints.iter().map(|endpoint| endpoint.weight))
        .and_then(|index| endpoints.get(index))
        .map(|endpoint| endpoint.addr)
}

fn choose_psm_index(entries: &[PsmEntry]) -> Option<usize> {
    choose_weighted_index(entries.iter().map(|entry| {
        if entry.weight <= 0.0 {
            1
        } else {
            entry.weight.round() as u32
        }
    }))
}

fn choose_weighted_index(weights: impl Iterator<Item = u32>) -> Option<usize> {
    let weights: Vec<u32> = weights.map(|weight| weight.max(1)).collect();
    if weights.is_empty() {
        return None;
    }
    WeightedIndex::new(&weights)
        .ok()
        .map(|dist| dist.sample(&mut rand::thread_rng()))
        .or(Some(0))
}

fn fill_blank_psm_entry(mut remote: PsmEntry, local: &PsmEntry) -> PsmEntry {
    if remote.psm.is_empty() {
        return local.clone();
    }
    if remote.idc.is_empty() {
        remote.idc = local.idc.clone();
    }
    if remote.cluster.is_empty() {
        remote.cluster = local.cluster.clone();
    }
    if remote.weight <= 0.0 {
        remote.weight = DEFAULT_LOCAL_PSM_WEIGHT;
    }
    remote
}

fn decode_psm_info(content: &[u8]) -> Result<Option<PsmInfo>, CliError> {
    let result: PsmResult = serde_json::from_slice(content)?;
    Ok(result.psm_info)
}

fn static_endpoints_from_env() -> Option<Vec<WeightedEndpoint>> {
    let addresses = std::env::var(TEST_TOSAPI_ADDR_ENV).ok()?;
    let endpoints: Vec<WeightedEndpoint> = addresses
        .split(';')
        .filter_map(|value| value.trim().parse::<SocketAddr>().ok())
        .map(|addr| WeightedEndpoint::new(addr, DEFAULT_ENDPOINT_WEIGHT))
        .collect();
    if endpoints.is_empty() {
        None
    } else {
        Some(endpoints)
    }
}

fn consul_addr_from_env() -> String {
    let host = consul_host_from_env();
    let port = std::env::var("CONSUL_HTTP_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_CONSUL_PORT);
    format!("{host}:{port}")
}

fn consul_host_from_env() -> String {
    for (name, is_ipv6) in [
        ("CONSUL_HTTP_HOST", false),
        ("MY_HOST_IP", false),
        ("TCE_HOST_IP", false),
        ("MY_HOST_IPV6", true),
    ] {
        if let Some(value) = non_empty_string(std::env::var(name).ok().as_deref()) {
            return if is_ipv6 { format!("[{value}]") } else { value };
        }
    }
    DEFAULT_CONSUL_HOST.to_string()
}

fn consul_timeout_ms() -> u64 {
    std::env::var("CONSUL_HTTP_TIMEOUT")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CONSUL_TIMEOUT_MS)
}

fn infer_addr_family_from_env() -> AddrFamilyOptions {
    let has_v4 = env_var_is_non_empty("BYTED_HOST_IP");
    let has_v6 = env_var_is_non_empty("BYTED_HOST_IPV6");
    match (has_v4, has_v6) {
        (true, true) => AddrFamilyOptions {
            addr_family: Some(AddrFamily::DualStack),
            unique: Some(AddrFamily::V6),
        },
        (true, false) => AddrFamilyOptions {
            addr_family: Some(AddrFamily::V4),
            unique: None,
        },
        (false, true) => AddrFamilyOptions {
            addr_family: Some(AddrFamily::V6),
            unique: None,
        },
        (false, false) => AddrFamilyOptions {
            addr_family: None,
            unique: None,
        },
    }
}

fn consul_service_name(request: &ConsulLookupRequest) -> String {
    match request.idc.as_deref() {
        Some(idc) if !request.psm.contains(".service.") => {
            format!("{}.service.{}", request.psm, idc)
        }
        _ => request.psm.clone(),
    }
}

fn choose_tosv_endpoint(idc: &str) -> &'static str {
    let idc = idc.to_ascii_lowercase();
    if idc.contains("boe") {
        return TOSV_BOE_ENDPOINT;
    }
    if idc.contains("iboe") {
        return TOSV_IBOE_ENDPOINT;
    }
    if idc.contains("sg") {
        return TOSV_SG_ENDPOINT;
    }
    if idc.contains("ttptx2") || idc.contains("ttp2") {
        return TOSV_TTP2_ENDPOINT;
    }
    if idc.contains("ttp") {
        return TOSV_TTP_ENDPOINT;
    }
    if idc.contains("aiso") {
        return TOSV_GISO_ENDPOINT;
    }
    TOSV_ENDPOINT
}

fn env_var_is_non_empty(name: &str) -> bool {
    std::env::var(name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn non_empty_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn empty_endpoint_error() -> CliError {
    CliError::TransferFailed("No available addresses from PSM resolver".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        choose_tosv_endpoint, consul_service_name, infer_addr_family_from_env, AddrFamily,
        ConsulLookupRequest, PsmDiscoveryConfig,
    };
    use crate::infra::config::Profile;
    use std::sync::Mutex;

    static DISCOVERY_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_discovery_env(run: impl FnOnce()) {
        let _guard = DISCOVERY_ENV_LOCK.lock().expect("discovery env lock");
        let old_v4 = std::env::var("BYTED_HOST_IP").ok();
        let old_v6 = std::env::var("BYTED_HOST_IPV6").ok();
        std::env::remove_var("BYTED_HOST_IP");
        std::env::remove_var("BYTED_HOST_IPV6");
        run();
        restore_env("BYTED_HOST_IP", old_v4);
        restore_env("BYTED_HOST_IPV6", old_v6);
    }

    fn restore_env(name: &str, value: Option<String>) {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }

    #[test]
    fn psm_discovery_config_requires_psm_before_modifiers_apply() {
        let no_psm = PsmDiscoveryConfig::from_profile(&Profile {
            idc: Some("boe".to_string()),
            cluster: Some("default".to_string()),
            addr_family: Some("v4".to_string()),
            ..Default::default()
        })
        .expect("config");
        assert!(no_psm.is_none());

        let with_psm = PsmDiscoveryConfig::from_profile(&Profile {
            psm: Some("tos.example.service".to_string()),
            idc: Some("boe".to_string()),
            cluster: Some("default".to_string()),
            addr_family: Some("dual_stack".to_string()),
            ..Default::default()
        })
        .expect("config")
        .expect("psm config");

        assert_eq!(with_psm.psm, "tos.example.service");
        assert_eq!(with_psm.idc, "boe");
        assert_eq!(with_psm.cluster, "default");
        assert_eq!(with_psm.addr_family, Some(AddrFamily::DualStack));
    }

    #[test]
    fn consul_name_adds_service_idc_suffix_only_when_needed() {
        let request = ConsulLookupRequest {
            psm: "tos.access.bns".to_string(),
            idc: Some("boe".to_string()),
            cluster: None,
            addr_family: None,
        };
        assert_eq!(consul_service_name(&request), "tos.access.bns.service.boe");

        let explicit = ConsulLookupRequest {
            psm: "tos.access.bns.service.boe".to_string(),
            ..request
        };
        assert_eq!(consul_service_name(&explicit), "tos.access.bns.service.boe");
    }

    #[test]
    fn addr_family_inference_matches_byted_sd() {
        with_discovery_env(|| {
            assert_eq!(infer_addr_family_from_env().addr_family, None);
            std::env::set_var("BYTED_HOST_IP", "10.0.0.1");
            assert_eq!(
                infer_addr_family_from_env().addr_family,
                Some(AddrFamily::V4)
            );
            std::env::set_var("BYTED_HOST_IPV6", "2605::1");
            let inferred = infer_addr_family_from_env();
            assert_eq!(inferred.addr_family, Some(AddrFamily::DualStack));
            assert_eq!(inferred.unique, Some(AddrFamily::V6));
        });
    }

    #[test]
    fn tosv_endpoint_selection_keeps_sdk_order() {
        assert_eq!(choose_tosv_endpoint("boe"), super::TOSV_BOE_ENDPOINT);
        assert_eq!(choose_tosv_endpoint("sg1"), super::TOSV_SG_ENDPOINT);
        assert_eq!(choose_tosv_endpoint("ttp2"), super::TOSV_TTP2_ENDPOINT);
        assert_eq!(choose_tosv_endpoint("cn"), super::TOSV_ENDPOINT);
    }
}
