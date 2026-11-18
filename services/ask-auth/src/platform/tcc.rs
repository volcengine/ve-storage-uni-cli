//! Fresh, authenticated TCC V2 snapshots over bounded BConfig V1 HTTP.
//! No SDK, background worker, or previous successful value is retained.
use crate::platform::config::{runtime::RuntimeContext, Locator};
use transport::HttpReader;
mod credentials;
mod transport;
use serde::{
    de::{value::MapAccessDeserializer, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const MAX_META_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONTENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_SELECTED_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 128;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(3);

/// Fixed, secret-safe categories; errors never retain upstream error bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TccError {
    /// Unsafe namespace/space syntax.
    InvalidLocator,
    /// Host environment does not match the explicit expected environment.
    Environment,
    /// Service identity or current DPS credential is absent or invalid.
    Identity,
    /// Discovery or authenticated HTTP read failed, without retaining its error.
    Unavailable,
    /// The complete snapshot read exceeded three seconds.
    Timeout,
    /// Resource, schema, bounds, gray configuration or version is invalid.
    InvalidSnapshot,
    /// A requested typed payload is absent from this snapshot.
    MissingConfiguration,
}
impl fmt::Display for TccError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLocator => "invalid TCC locator",
            Self::Environment => "TCC environment unavailable",
            Self::Identity => "TCC workload identity unavailable",
            Self::Unavailable => "TCC unavailable",
            Self::Timeout => "TCC snapshot timed out",
            Self::InvalidSnapshot => "invalid TCC snapshot",
            Self::MissingConfiguration => "required TCC configuration missing",
        })
    }
}
impl std::error::Error for TccError {}

/// The only configuration payloads exposed to the trusted host parser.
#[derive(Clone, Copy)]
pub enum PayloadKey {
    /// ask_auth_runtime
    Runtime,
    /// ask_auth_secrets
    Secrets,
    /// ask_auth_admission
    Admission,
    /// ask_auth_cleanup
    Cleanup,
}
impl PayloadKey {
    fn name(self) -> &'static str {
        match self {
            Self::Runtime => "ask_auth_runtime",
            Self::Secrets => "ask_auth_secrets",
            Self::Admission => "ask_auth_admission",
            Self::Cleanup => "ask_auth_cleanup",
        }
    }
}

/// A single validated read; deliberately implements neither Debug nor Display.
pub struct Snapshot {
    selected: BTreeMap<String, SensitiveText>,
    version: String,
    read_completed_at: Instant,
}
impl Snapshot {
    /// Borrow a selected payload for trusted parsing; returns `MissingConfiguration` if absent.
    /// The borrow is bounded by this snapshot's lifetime; no map export is provided.
    pub fn required(&self, key: PayloadKey) -> Result<&str, TccError> {
        self.selected
            .get(key.name())
            .map(SensitiveText::borrow)
            .ok_or(TccError::MissingConfiguration)
    }
    /// Return the validated version code. Performs no I/O and cannot fail.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Return monotonic read-completion time for host freshness checks; cannot fail.
    pub fn read_completed_at(&self) -> Instant {
        self.read_completed_at
    }
}

struct SensitiveText(Zeroizing<String>);
impl SensitiveText {
    fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}
impl<'de> Deserialize<'de> for SensitiveText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}
struct RawDocument {
    key: String,
    value: SensitiveText,
}
type ReadFuture<'a> = Pin<Box<dyn Future<Output = Result<RawDocument, TccError>> + Send + 'a>>;
trait RawReader {
    fn read<'a>(&'a self, key: &'a str) -> ReadFuture<'a>;
}
/// Authenticated HTTP source with no cached snapshot or public endpoint override.
pub struct TccSource {
    locator: Locator,
    reader: HttpReader,
}
impl TccSource {
    /// Construct a client for `locator` under Tokio, using validated platform metadata.
    /// Performs no network I/O or worker startup. Returns fixed environment, identity,
    /// or availability errors when runtime metadata, credential source, or client is invalid.
    pub fn new(locator: Locator) -> Result<Self, TccError> {
        tokio::runtime::Handle::try_current().map_err(|_| TccError::Unavailable)?;
        let context = RuntimeContext::from_env(locator.environment)?;
        Ok(Self {
            locator,
            reader: HttpReader::new(context)?,
        })
    }
    /// Fetch metadata and content afresh within one three-second acceptance deadline.
    /// Returns a validated matching snapshot or fixed Timeout, Identity, Unavailable,
    /// or InvalidSnapshot errors. Synchronous parsing is checked for late completion.
    pub async fn read_snapshot(&self) -> Result<Snapshot, TccError> {
        read_snapshot(&self.reader, &self.locator).await
    }
}

async fn read_snapshot(reader: &impl RawReader, locator: &Locator) -> Result<Snapshot, TccError> {
    // [Review Fix #61] Bound discovery, credentials, and both reads together and always fetch data even when versions repeat.
    let deadline = tokio::time::Instant::now() + SNAPSHOT_TIMEOUT;
    let snapshot = tokio::time::timeout_at(deadline, async {
        let meta = reader
            .read(&format!("/tcc/v2/meta/{}", locator.namespace))
            .await?;
        let content = reader
            .read(&format!(
                "/tcc/v2/data/{}/{}",
                locator.namespace, locator.space
            ))
            .await?;
        decode_snapshot(meta, content, locator)
    })
    .await
    .map_err(|_| TccError::Timeout)??;
    // [Review Fix #68] Tokio cannot preempt synchronous parsing work; reject completion past the deadline.
    if tokio::time::Instant::now() >= deadline {
        return Err(TccError::Timeout);
    }
    Ok(snapshot)
}

type Meta = ObjectOnly<MetaFields>;
type SpaceVersion = ObjectOnly<SpaceVersionFields>;
type Content = ObjectOnly<ContentFields>;
type GrayData = ObjectOnly<GrayDataFields>;

// [Review Fix #65] Derived Serde structs accept positional arrays; the wire contract requires objects.
struct ObjectOnly<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for ObjectOnly<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}
struct ObjectVisitor<T>(PhantomData<T>);
impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
    type Value = ObjectOnly<T>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a configuration object")
    }
    fn visit_map<A: MapAccess<'de>>(self, entries: A) -> Result<Self::Value, A::Error> {
        T::deserialize(MapAccessDeserializer::new(entries)).map(ObjectOnly)
    }
}

#[derive(Deserialize)]
struct MetaFields {
    version_codes: Vec<SpaceVersion>,
}
#[derive(Deserialize)]
struct SpaceVersionFields {
    confspace: String,
    version_code: String,
}
#[derive(Deserialize)]
struct ContentFields {
    version_code: String,
    data: PayloadMap,
    gray_data: Option<GrayData>,
}
#[derive(Deserialize)]
struct GrayDataFields {
    #[serde(default)]
    data: PayloadMap,
    gray_code: Option<SensitiveText>,
    gray_ip_list: Option<Vec<SensitiveText>>,
}
impl GrayDataFields {
    fn is_empty(&self) -> bool {
        self.data.0.is_empty()
            && self
                .gray_code
                .as_ref()
                .is_none_or(|code| code.borrow().is_empty())
            && self.gray_ip_list.as_ref().is_none_or(Vec::is_empty)
    }
}
#[derive(Default)]
struct PayloadMap(BTreeMap<String, SensitiveText>);
impl<'de> Deserialize<'de> for PayloadMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(PayloadMapVisitor)
    }
}
struct PayloadMapVisitor;
impl<'de> Visitor<'de> for PayloadMapVisitor {
    type Value = PayloadMap;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded unique string configuration map")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Self::Value, A::Error> {
        let mut values = BTreeMap::new();
        while let Some((key, value)) = entries.next_entry::<String, SensitiveText>()? {
            // [Review Fix #62] Reject duplicates rather than silently replacing a secret payload.
            if values.len() >= MAX_ENTRIES || values.contains_key(&key) {
                return Err(serde::de::Error::custom("invalid configuration map"));
            }
            values.insert(key, value);
        }
        Ok(PayloadMap(values))
    }
}

fn decode_snapshot(
    meta: RawDocument,
    content: RawDocument,
    locator: &Locator,
) -> Result<Snapshot, TccError> {
    check_document(
        &meta,
        &format!("/tcc/v2/meta/{}", locator.namespace),
        MAX_META_BYTES,
    )?;
    check_document(
        &content,
        &format!("/tcc/v2/data/{}/{}", locator.namespace, locator.space),
        MAX_CONTENT_BYTES,
    )?;
    let metadata: Meta =
        serde_json::from_str(meta.value.borrow()).map_err(|_| TccError::InvalidSnapshot)?;
    let payloads: Content =
        serde_json::from_str(content.value.borrow()).map_err(|_| TccError::InvalidSnapshot)?;
    let payloads = payloads.0;
    let version = selected_version(metadata, &locator.space)?;
    if payloads.version_code != version
        || payloads
            .gray_data
            .as_ref()
            .is_some_and(|gray| !gray.0.is_empty())
    {
        return Err(TccError::InvalidSnapshot);
    }
    let selected = select_payloads(payloads.data)?;
    Ok(Snapshot {
        selected,
        version,
        read_completed_at: Instant::now(),
    })
}
fn check_document(
    document: &RawDocument,
    expected_key: &str,
    max_bytes: usize,
) -> Result<(), TccError> {
    // [Review Fix #63] Permit only the SDK's single-leading-slash normalization.
    if document.key.strip_prefix('/').unwrap_or(&document.key)
        != expected_key.trim_start_matches('/')
        || document.value.borrow().len() > max_bytes
    {
        return Err(TccError::InvalidSnapshot);
    }
    // [Review Fix #67] Bound unknown subtrees too, before either metadata/content reaches typed Serde parsing.
    crate::json::check_json_depth(document.value.borrow().as_bytes())
        .map_err(|_| TccError::InvalidSnapshot)
}
fn selected_version(metadata: Meta, space: &str) -> Result<String, TccError> {
    if metadata.0.version_codes.len() > MAX_ENTRIES {
        return Err(TccError::InvalidSnapshot);
    }
    let mut matches = metadata
        .0
        .version_codes
        .into_iter()
        .map(|entry| entry.0)
        .filter(|entry| entry.confspace == space);
    let selected = matches.next().ok_or(TccError::InvalidSnapshot)?;
    if matches.next().is_some()
        || selected.version_code.is_empty()
        || selected.version_code.len() > 256
        || selected.version_code.chars().any(char::is_control)
    {
        return Err(TccError::InvalidSnapshot);
    }
    Ok(selected.version_code)
}
fn select_payloads(mut payloads: PayloadMap) -> Result<BTreeMap<String, SensitiveText>, TccError> {
    let mut selected = BTreeMap::new();
    let mut selected_bytes = 0;
    for key in [
        PayloadKey::Runtime,
        PayloadKey::Secrets,
        PayloadKey::Admission,
        PayloadKey::Cleanup,
    ] {
        if let Some(payload) = payloads.0.remove(key.name()) {
            selected_bytes += payload.borrow().len();
            if payload.borrow().len() > MAX_PAYLOAD_BYTES || selected_bytes > MAX_SELECTED_BYTES {
                return Err(TccError::InvalidSnapshot);
            }
            selected.insert(key.name().into(), payload);
        }
    }
    Ok(selected)
}
#[cfg(test)]
mod tests;
