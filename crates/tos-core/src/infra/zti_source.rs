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

//! Public ByteTOS ZTI source selection and token refresh.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use tokio::{
    fs,
    io::AsyncReadExt,
    sync::{Mutex, OnceCell, RwLock},
};
use tokio_util::sync::CancellationToken;

use super::{
    zti_agent,
    zti_credentials::{ZtiTokenError, ZtiTokenProvider},
};

const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TOKEN_BYTES: u64 = 64 * 1024;
const AGENT_CACHE_SECONDS: u64 = 600;
const REFRESH_RETRY_SECONDS: u64 = 2;
const EXPIRATION_HEADROOM_SECONDS: u64 = 10;
const DEFAULT_AGENT_SOCKET: &str = "/run/zti-agent.sock";

/// Report the highest-priority locally configured token source without reading it.
///
/// # Returns
///
/// `environment_string`, `agent`, or `file` when the corresponding source is
/// present, otherwise `None`. Presence does not prove token validity, Agent
/// connectivity, or ByteTOS service authorization.
///
/// # Errors
///
/// This best-effort offline inspection does not return errors.
pub fn configured_source_kind() -> Option<&'static str> {
    if std::env::var("SEC_TOKEN_STRING").is_ok() {
        return Some("environment_string");
    }
    // [Review Fix #31] ZTI Client ignores paths that are not valid UTF-8.
    let configured_agent = std::env::var("ZTI_AGENT_SOCKET_PATH")
        .ok()
        .map(PathBuf::from);
    // [Review Fix #5] The Agent transport is Unix-only; avoid reporting it as usable on Windows.
    if cfg!(unix)
        && (configured_agent.is_some_and(|path| path.exists())
            || Path::new(DEFAULT_AGENT_SOCKET).exists())
    {
        return Some("agent");
    }
    std::env::var("SEC_TOKEN_PATH")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .map(|_| "file")
}

enum TokenSource {
    Embedded(String),
    Agent {
        path: PathBuf,
        cache: OnceCell<Arc<RefreshState>>,
    },
    File {
        path: PathBuf,
        cache: OnceCell<Arc<RefreshState>>,
    },
}

struct CachedToken {
    token: String,
    expires_at: u64,
}

impl CachedToken {
    fn refresh_delay(&self, now: u64) -> Duration {
        let before_expiration = self
            .expires_at
            .saturating_sub(now)
            .saturating_sub(EXPIRATION_HEADROOM_SECONDS);
        Duration::from_secs(AGENT_CACHE_SECONDS.min(before_expiration).max(1))
    }
}

struct RefreshState {
    current: RwLock<CachedToken>,
    refresh_lock: Mutex<()>,
    cancellation: CancellationToken,
}

impl RefreshState {
    fn new(token: CachedToken) -> Self {
        Self {
            current: RwLock::new(token),
            refresh_lock: Mutex::new(()),
            cancellation: CancellationToken::new(),
        }
    }

    async fn token(&self) -> Result<String, ZtiTokenError> {
        let current = self.current.read().await;
        if unix_time()? >= current.expires_at {
            return Err(ZtiTokenError::Expired);
        }
        Ok(current.token.clone())
    }
}

impl Drop for RefreshState {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

/// Create the public ByteTOS ZTI provider without reading credentials yet.
///
/// Discovery happens on the first real ZTI request. Clones share source selection
/// and the Agent refresh cache; files are re-read on each request and retain a
/// valid last-good token through temporary read failures. Failures return the
/// closed [`ZtiTokenError`] categories, never raw tokens or transport errors.
///
/// # Returns
///
/// An invocation-scoped provider that resolves a current JWT-SVID per request.
///
/// # Errors
///
/// Construction is infallible. Token resolution reports errors through the
/// provider's `get_header_value` method.
pub fn provider() -> ZtiTokenProvider {
    let source = Arc::new(OnceCell::<TokenSource>::new());
    // [Review Fix #7] Preserve source provenance with the resolver itself so
    // MCP cannot mistake a caller-supplied provider for built-in discovery.
    ZtiTokenProvider::from_builtin_source(move || {
        let source = Arc::clone(&source);
        async move {
            tokio::time::timeout(TOKEN_TIMEOUT, async {
                source
                    .get_or_try_init(discover_source)
                    .await?
                    .current_token()
                    .await
            })
            .await
            .map_err(|_| ZtiTokenError::Timeout)?
        }
    })
}

async fn discover_source() -> Result<TokenSource, ZtiTokenError> {
    let embedded = std::env::var("SEC_TOKEN_STRING").ok();
    // [Review Fix #3] A selected higher-priority source must not inspect lower-priority paths.
    if let Some(token) = embedded {
        return Ok(TokenSource::Embedded(token));
    }
    let configured_agent = std::env::var("ZTI_AGENT_SOCKET_PATH")
        .ok()
        .map(PathBuf::from);
    let default_agent = PathBuf::from(DEFAULT_AGENT_SOCKET);
    // [Review Fix #4] Skip Unix Agent discovery on Windows so a configured
    // socket path cannot prevent the supported file source from being chosen.
    let agent = if cfg!(unix) {
        match configured_agent {
            Some(path) if fs::metadata(&path).await.is_ok() => Some(path),
            _ if fs::metadata(&default_agent).await.is_ok() => Some(default_agent),
            _ => None,
        }
    } else {
        None
    };
    if let Some(path) = agent {
        return select_source(None, Some(path), None, cfg!(unix));
    }
    let file = std::env::var("SEC_TOKEN_PATH").ok().map(PathBuf::from);
    let file = match file {
        Some(path) if fs::metadata(&path).await.is_ok() => Some(path),
        _ => None,
    };
    select_source(None, None, file, cfg!(unix))
}

fn select_source(
    embedded: Option<String>,
    agent: Option<PathBuf>,
    file: Option<PathBuf>,
    supports_agent: bool,
) -> Result<TokenSource, ZtiTokenError> {
    if let Some(token) = embedded {
        return Ok(TokenSource::Embedded(token));
    }
    if let Some(path) = agent.filter(|_| supports_agent) {
        return Ok(TokenSource::Agent {
            path,
            cache: OnceCell::new(),
        });
    }
    file.map(|path| TokenSource::File {
        path,
        cache: OnceCell::new(),
    })
    .ok_or(ZtiTokenError::Unavailable)
}

impl TokenSource {
    async fn current_token(&self) -> Result<String, ZtiTokenError> {
        match self {
            Self::Embedded(token) => {
                token_expiration(token, unix_time()?)?;
                Ok(token.clone())
            }
            Self::File { path, cache } => current_file_token(path, cache).await,
            Self::Agent { path, cache } => current_agent_token(path, cache).await,
        }
    }
}

async fn current_agent_token(
    path: &Path,
    cache: &OnceCell<Arc<RefreshState>>,
) -> Result<String, ZtiTokenError> {
    let cache = cache
        .get_or_try_init(|| async { init_agent_refresh(path).await })
        .await?;
    agent_token_with_refresh(cache, || fetch_agent_token(path)).await
}

async fn agent_token_with_refresh<F, Fut>(
    cache: &RefreshState,
    fetch: F,
) -> Result<String, ZtiTokenError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<CachedToken, ZtiTokenError>>,
{
    match cache.token().await {
        Ok(token) => return Ok(token),
        Err(ZtiTokenError::Expired) => {}
        Err(error) => return Err(error),
    }
    // [Review Fix #28] Recover when a former runtime stopped the background Agent task.
    let _guard = cache.refresh_lock.lock().await;
    match cache.token().await {
        Ok(token) => return Ok(token),
        Err(ZtiTokenError::Expired) => {}
        Err(error) => return Err(error),
    }
    let refreshed = fetch().await?;
    let token = refreshed.token.clone();
    *cache.current.write().await = refreshed;
    Ok(token)
}

async fn current_file_token(
    path: &Path,
    cache: &OnceCell<Arc<RefreshState>>,
) -> Result<String, ZtiTokenError> {
    if let Some(cache) = cache.get() {
        let _guard = cache.refresh_lock.lock().await;
        // [Review Fix #25] Preserve immediate file rotation without an SDK-style polling delay.
        if let Ok(token) = fetch_file_token(path).await {
            // [Review Fix #29] Return this read, even if another refresh is queued.
            let selected = token.token.clone();
            *cache.current.write().await = token;
            return Ok(selected);
        }
        return cache.token().await;
    }
    cache
        .get_or_try_init(|| async {
            let initial = fetch_file_token(path).await?;
            Ok(Arc::new(RefreshState::new(initial)))
        })
        .await?
        .token()
        .await
}

async fn init_agent_refresh(path: &Path) -> Result<Arc<RefreshState>, ZtiTokenError> {
    let initial = fetch_agent_token(path).await?;
    let cache = Arc::new(RefreshState::new(initial));
    tokio::spawn(refresh_agent_loop(
        path.to_path_buf(),
        Arc::downgrade(&cache),
    ));
    Ok(cache)
}

async fn fetch_agent_token(path: &Path) -> Result<CachedToken, ZtiTokenError> {
    let token = zti_agent::fetch_token(path).await?;
    let expires_at = token_expiration(&token, unix_time()?)?;
    Ok(CachedToken { token, expires_at })
}

async fn refresh_agent_loop(path: PathBuf, weak_cache: std::sync::Weak<RefreshState>) {
    let Some(cache) = weak_cache.upgrade() else {
        return;
    };
    let cancellation = cache.cancellation.clone();
    let mut delay = cache
        .current
        .read()
        .await
        .refresh_delay(unix_time().unwrap_or(0));
    drop(cache);
    loop {
        tokio::select! {
            _ = cancellation.cancelled() => return,
            _ = tokio::time::sleep(delay) => {},
        }
        let Some(cache) = weak_cache.upgrade() else {
            return;
        };
        let _guard = cache.refresh_lock.lock().await;
        match fetch_agent_token(&path).await {
            Ok(token) => {
                delay = token.refresh_delay(unix_time().unwrap_or(0));
                *cache.current.write().await = token;
            }
            Err(_) => delay = Duration::from_secs(REFRESH_RETRY_SECONDS),
        }
    }
}

async fn fetch_file_token(path: &Path) -> Result<CachedToken, ZtiTokenError> {
    let token = read_file(path).await?;
    let expires_at = token_expiration(&token, unix_time()?)?;
    Ok(CachedToken { token, expires_at })
}

async fn read_file(path: &Path) -> Result<String, ZtiTokenError> {
    let metadata = fs::metadata(path)
        .await
        .map_err(|_| ZtiTokenError::Unavailable)?;
    if !metadata.is_file() || metadata.len() > MAX_TOKEN_BYTES {
        return Err(ZtiTokenError::Unavailable);
    }
    let file = fs::File::open(path)
        .await
        .map_err(|_| ZtiTokenError::Unavailable)?;
    let mut limited = file.take(MAX_TOKEN_BYTES + 1);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| ZtiTokenError::Unavailable)?;
    if bytes.len() as u64 > MAX_TOKEN_BYTES {
        return Err(ZtiTokenError::Unavailable);
    }
    String::from_utf8(bytes).map_err(|_| ZtiTokenError::Unavailable)
}

#[derive(Deserialize)]
struct JwtClaims {
    sub: String,
    exp: u64,
}

fn token_expiration(token: &str, now: u64) -> Result<u64, ZtiTokenError> {
    if token.len() as u64 > MAX_TOKEN_BYTES {
        return Err(ZtiTokenError::Unavailable);
    }
    let mut segments = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(ZtiTokenError::Unavailable);
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err(ZtiTokenError::Unavailable);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| ZtiTokenError::Unavailable)?;
    let claims: JwtClaims =
        serde_json::from_slice(&decoded).map_err(|_| ZtiTokenError::Unavailable)?;
    if !is_valid_bytedance_spiffe_subject(&claims.sub) {
        return Err(ZtiTokenError::Unavailable);
    }
    if now >= claims.exp {
        Err(ZtiTokenError::Expired)
    } else {
        Ok(claims.exp)
    }
}

fn is_valid_bytedance_spiffe_subject(subject: &str) -> bool {
    if !subject.is_ascii() {
        return false;
    }
    let Some((trust_domain, path)) = subject
        .strip_prefix("spiffe://")
        .and_then(|remainder| remainder.split_once('/'))
    else {
        return false;
    };
    if !is_valid_spiffe_trust_domain(trust_domain) {
        return false;
    }

    let mut value_lengths = [None; 4];
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        let Some((key, value)) = segment.split_once(':') else {
            return false;
        };
        let key_index = match key {
            "ns" => 0,
            "r" => 1,
            "vdc" => 2,
            "id" => 3,
            _ => return false,
        };
        // [Review Fix #20] The SDK parser accepts empty segment values, including optional r:.
        if value_lengths[key_index].is_some()
            || !value.bytes().all(|character| {
                matches!(character, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.')
            })
        {
            return false;
        }
        value_lengths[key_index] = Some(value.len());
    }
    if value_lengths[0].is_none() || value_lengths[3].is_none() {
        return false;
    }
    // [Review Fix #23] Keep parsing and canonical length checks separate and within one responsibility.
    is_spiffe_id_within_length_limit(trust_domain, value_lengths)
}

fn is_spiffe_id_within_length_limit(trust_domain: &str, value_lengths: [Option<usize>; 4]) -> bool {
    // [Review Fix #22] The SDK compacts values in ns, r, vdc, id order before checking length.
    let mut compacted_value_length = 0;
    // [Review Fix #19] Repeated slashes are omitted from the normalized SPIFFE ID.
    let mut canonical_length = "spiffe://".len() + trust_domain.len();
    for (value_length, key_length) in value_lengths.into_iter().zip([2, 1, 3, 2]) {
        if let Some(value_length) = value_length {
            compacted_value_length += value_length;
            // [Review Fix #21] Leading empty values have no end index in spiffe-id's compact form.
            if compacted_value_length > 0 {
                canonical_length += key_length + value_length + 2;
            }
        }
    }
    canonical_length <= 2048
}

fn is_valid_spiffe_trust_domain(trust_domain: &str) -> bool {
    !trust_domain.is_empty()
        && trust_domain.len() <= 255
        && trust_domain
            .bytes()
            .all(|character| matches!(character, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.'))
}

fn unix_time() -> Result<u64, ZtiTokenError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ZtiTokenError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn token(expires_at: u64, marker: &str) -> String {
        let claims = serde_json::json!({
            "sub": "spiffe://example.org/ns:test/id:integration",
            "exp": expires_at,
        });
        format!(
            "e30.{}.{}",
            URL_SAFE_NO_PAD.encode(claims.to_string()),
            marker
        )
    }

    #[test]
    fn jwt_expiration_boundary_is_checked_without_verifying_signature() {
        assert_eq!(token_expiration(&token(101, "a"), 100).unwrap(), 101);
        assert_eq!(
            token_expiration(&token(100, "b"), 100),
            Err(ZtiTokenError::Expired)
        );
        assert_eq!(
            token_expiration("invalid-secret", 100),
            Err(ZtiTokenError::Unavailable)
        );
    }

    #[test]
    fn jwt_subject_requires_bytedance_spiffe_identity() {
        let valid = token(101, "valid");
        assert_eq!(token_expiration(&valid, 100), Ok(101));

        for subject in [
            "spiffe://example.org/ns:test/r:/id:integration",
            "spiffe://example.org/ns:/id:integration",
        ] {
            let claims = serde_json::json!({"sub": subject, "exp": 101});
            let candidate = format!(
                "e30.{}.signature",
                URL_SAFE_NO_PAD.encode(claims.to_string())
            );
            assert_eq!(
                token_expiration(&candidate, 100),
                Ok(101),
                "subject={subject}"
            );
        }

        for subject in [
            "spiffe://example.org",
            "spiffe://example.org/ns:test",
            "spiffe://example.org/id:integration",
            "spiffe://EXAMPLE.ORG/ns:test/id:integration",
            "spiffe://example.org/ns:test/ns:duplicate/id:integration",
            "spiffe://example.org/ns:test/unknown:value/id:integration",
            "spiffe://example.org/ns:test/id:bad?value",
        ] {
            let claims = serde_json::json!({"sub": subject, "exp": 101});
            let candidate = format!(
                "e30.{}.signature",
                URL_SAFE_NO_PAD.encode(claims.to_string())
            );
            assert_eq!(
                token_expiration(&candidate, 100),
                Err(ZtiTokenError::Unavailable),
                "subject={subject}"
            );
        }
    }

    #[test]
    fn jwt_subject_length_uses_normalized_spiffe_path() {
        let repeated_slashes = "/".repeat(2100);
        let subject = format!("spiffe://example.org/ns:test{repeated_slashes}id:integration");
        let claims = serde_json::json!({"sub": subject, "exp": 101});
        let candidate = format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        assert_eq!(token_expiration(&candidate, 100), Ok(101));
    }

    #[test]
    fn jwt_subject_length_omits_leading_empty_components() {
        let identity = "a".repeat(2034);
        let subject = format!("spiffe://d/ns:/id:{identity}");
        let claims = serde_json::json!({"sub": subject, "exp": 101});
        let candidate = format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        assert_eq!(token_expiration(&candidate, 100), Ok(101));

        let identity = "a".repeat(2035);
        let subject = format!("spiffe://d/ns:/id:{identity}");
        let claims = serde_json::json!({"sub": subject, "exp": 101});
        let candidate = format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        assert_eq!(
            token_expiration(&candidate, 100),
            Err(ZtiTokenError::Unavailable)
        );
    }

    #[test]
    fn jwt_subject_length_uses_canonical_key_order() {
        let identity = "a".repeat(2031);
        let rejected_subject = format!("spiffe://d/id:/ns:{identity}");
        let accepted_subject = format!("spiffe://d/id:{identity}/ns:");
        for (subject, expected) in [
            (rejected_subject, Err(ZtiTokenError::Unavailable)),
            (accepted_subject, Ok(101)),
        ] {
            let claims = serde_json::json!({"sub": subject, "exp": 101});
            let candidate = format!(
                "e30.{}.signature",
                URL_SAFE_NO_PAD.encode(claims.to_string())
            );
            assert_eq!(token_expiration(&candidate, 100), expected);
        }
    }

    #[test]
    fn agent_refresh_is_scheduled_at_ten_minutes_or_expiration_headroom() {
        let cached = CachedToken {
            token: "redacted".into(),
            expires_at: 2_000,
        };
        assert_eq!(cached.refresh_delay(1_000), Duration::from_secs(600));

        let expiring = CachedToken {
            expires_at: 1_100,
            ..cached
        };
        assert_eq!(expiring.refresh_delay(1_000), Duration::from_secs(90));
        assert_eq!(expiring.refresh_delay(1_095), Duration::from_secs(1));
    }

    #[tokio::test]
    async fn expired_agent_cache_fetches_again_on_request() {
        let now = unix_time().unwrap();
        let next = token(now + 3600, "fresh");
        let cache = RefreshState::new(CachedToken {
            token: token(now - 1, "expired"),
            expires_at: now - 1,
        });
        let fetched = agent_token_with_refresh(&cache, || async {
            Ok(CachedToken {
                token: next.clone(),
                expires_at: now + 3600,
            })
        })
        .await
        .unwrap();
        assert_eq!(fetched, next);
    }

    #[tokio::test]
    async fn concurrent_expired_agent_requests_share_one_refresh() {
        let now = unix_time().unwrap();
        let cache = Arc::new(RefreshState::new(CachedToken {
            token: token(now - 1, "expired"),
            expires_at: now - 1,
        }));
        let fetch_count = Arc::new(AtomicU64::new(0));
        let fresh = token(now + 3600, "fresh");
        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
        let first_cache = Arc::clone(&cache);
        let first_count = Arc::clone(&fetch_count);
        let first_token = fresh.clone();
        let first = tokio::spawn(async move {
            agent_token_with_refresh(&first_cache, || async {
                first_count.fetch_add(1, Ordering::Relaxed);
                started_sender.send(()).unwrap();
                release_receiver.await.unwrap();
                Ok(CachedToken {
                    token: first_token,
                    expires_at: now + 3600,
                })
            })
            .await
        });
        started_receiver.await.unwrap();
        let second_cache = Arc::clone(&cache);
        let second_count = Arc::clone(&fetch_count);
        let second = tokio::spawn(async move {
            agent_token_with_refresh(&second_cache, || async {
                second_count.fetch_add(1, Ordering::Relaxed);
                Ok(CachedToken {
                    token: fresh,
                    expires_at: now + 3600,
                })
            })
            .await
        });
        tokio::pin!(second);
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut second)
            .await
            .is_err());
        release_sender.send(()).unwrap();
        assert_eq!(
            first.await.unwrap().unwrap(),
            second.await.unwrap().unwrap()
        );
        assert_eq!(fetch_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn file_source_retains_last_good_token_and_rotates_on_request() {
        let directory = std::env::temp_dir().join(format!(
            "public-zti-file-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        tokio::fs::create_dir(&directory).await.unwrap();
        let path = directory.join("token.jwt");
        let now = unix_time().unwrap();
        let first = token(now + 3600, "first");
        let second = token(now + 3600, "second");
        tokio::fs::write(&path, &first).await.unwrap();
        let source = TokenSource::File {
            path: path.clone(),
            cache: OnceCell::new(),
        };
        assert_eq!(source.current_token().await.unwrap(), first);
        tokio::fs::remove_file(&path).await.unwrap();
        assert_eq!(source.current_token().await.unwrap(), first);
        let replacement = directory.join("next.jwt");
        tokio::fs::write(&replacement, &second).await.unwrap();
        tokio::fs::rename(&replacement, &path).await.unwrap();
        assert_eq!(source.current_token().await.unwrap(), second);
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn file_source_rejects_expired_cache_when_file_is_missing() {
        let now = unix_time().unwrap();
        let path = std::env::temp_dir().join(format!(
            "missing-public-zti-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = OnceCell::new();
        assert!(cache
            .set(Arc::new(RefreshState::new(CachedToken {
                token: token(now - 1, "expired"),
                expires_at: now - 1,
            })))
            .is_ok());
        let source = TokenSource::File { path, cache };
        assert_eq!(source.current_token().await, Err(ZtiTokenError::Expired));
    }

    #[test]
    fn source_priority_is_embedded_then_agent_then_file() {
        let agent = std::path::PathBuf::from("/not-a-live-agent.sock");
        let file = std::path::PathBuf::from("/not-a-live-token.jwt");
        assert!(matches!(
            select_source(
                Some("string".into()),
                Some(agent.clone()),
                Some(file.clone()),
                true,
            )
            .unwrap(),
            TokenSource::Embedded(_)
        ));
        assert!(matches!(
            select_source(None, Some(agent), Some(file.clone()), true).unwrap(),
            TokenSource::Agent { .. }
        ));
        assert!(matches!(
            select_source(None, None, Some(file), true).unwrap(),
            TokenSource::File { .. }
        ));
        assert!(select_source(None, None, None, true).is_err());
    }

    #[test]
    fn unsupported_agent_does_not_block_file_token() {
        let agent = PathBuf::from("agent.sock");
        let file = PathBuf::from("token.jwt");
        assert!(matches!(
            select_source(None, Some(agent), Some(file), false).unwrap(),
            TokenSource::File { .. }
        ));
    }

    #[test]
    fn built_in_provider_keeps_its_source_identity() {
        let built_in = provider();
        assert!(!built_in.is_caller_supplied());
        assert!(!built_in.clone().is_caller_supplied());
    }
}
