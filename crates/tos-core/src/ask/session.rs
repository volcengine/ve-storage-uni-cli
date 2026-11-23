//! One-shot Doubao sessions using only an explicitly supplied user access token.
//! This client never executes commands, downloads artifacts, or approves tools.
//! Rejecting tool frames cannot prevent actions already performed by the cloud agent.
use super::Secret;
use reqwest::{header, Response, StatusCode};
use serde::Deserialize;
use std::time::Duration;
use url::Url;

mod protocol;
mod sse;

const BASE_URL: &str = "https://open.doubao.com/open-apis/agent";
const MAX_PROMPT_BYTES: usize = 64 * 1024;
// [Review Fix #3] The echoed prompt may expand sixfold in JSON, plus session metadata.
const MAX_CREATE_BYTES: usize = 512 * 1024;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(120);

/// A bounded fixed-endpoint client; no application secret, redirects, or retries.
pub struct SessionClient {
    http: reqwest::Client,
    base: Url,
    timeout: Duration,
}

/// Validated identifiers and committed text from one completed assistant reply.
pub struct Reply {
    /// Identifier of the newly created session.
    pub session_id: String,
    /// Identifier of the caller's initial user message, used to address MessageStream.
    pub user_message_id: String,
    /// Committed text items in sequence order, with previews and reasoning excluded.
    pub text: String,
}

/// Redacted session failures; provider bodies, credentials and prompts are never included.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SessionError {
    /// The local HTTPS transport could not be initialized.
    #[error("Doubao session transport could not be initialized")]
    Configuration,
    /// The prompt or access token is empty, oversized or invalid.
    #[error("Doubao question or access token is invalid")]
    Input,
    /// The server rejected the token; signing in again may be necessary.
    #[error("Doubao access was denied; login may be required")]
    Unauthorized,
    /// Application permissions, user scopes or allowlisting do not permit this operation.
    #[error("Doubao permission was denied; verify application permissions and user scopes")]
    PermissionDenied,
    /// The server reported quota pressure; no request was retried.
    #[error("Doubao rate limit was reached; the question was not retried")]
    RateLimited,
    /// A request failed; an uncertain session creation is deliberately never retried.
    #[error("Doubao session request failed; it was not retried")]
    Provider,
    /// The response violated the documented protocol or local resource limits.
    #[error("Doubao session response was invalid or exceeded local limits")]
    Response,
    /// The operation exceeded its total wall-clock deadline.
    #[error("Doubao session operation timed out; it was not retried")]
    Timeout,
    /// An observed cloud tool or artifact is unsupported in this recommendation-only client.
    #[error("Doubao returned a tool or artifact that this client does not support")]
    Unsupported,
    /// The assistant failed, was cancelled, or the stream ended without valid completion.
    #[error("Doubao reply did not complete successfully")]
    Incomplete,
}

type Result<T> = std::result::Result<T, SessionError>;

impl SessionClient {
    /// Construct a system-root HTTPS client with a fixed Doubao endpoint.
    /// Returns `Configuration` if TLS or HTTP transport initialization fails.
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: transport(true, OPERATION_TIMEOUT)?,
            base: Url::parse(BASE_URL).map_err(|_| SessionError::Configuration)?,
            timeout: OPERATION_TIMEOUT,
        })
    }

    /// Ask one question using the borrowed user `token` and caller-provided `prompt` only.
    /// Returns validated identifiers and final committed text after matching completion and EOF.
    /// Returns redacted input, HTTP, timeout, protocol, unsupported-content or completion errors.
    /// An uncertain POST is never retried, and a failed stream never creates another session.
    pub async fn ask(&self, token: &Secret, prompt: &str) -> Result<Reply> {
        validate_input(token, prompt)?;
        tokio::time::timeout(self.timeout, self.ask_once(token, prompt))
            .await
            .map_err(|_| SessionError::Timeout)?
    }

    async fn ask_once(&self, token: &Secret, prompt: &str) -> Result<Reply> {
        let created = self.create(token, prompt).await?;
        let url = self.endpoint(&["v1", "sessions", &created.id, "events", &created.user_id])?;
        let response = self
            .http
            .get(url)
            .bearer_auth(token.expose())
            .header(header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(http_error)?;
        check_response(&response, StatusCode::OK, "text/event-stream")?;
        let text = sse::read(response, &created.id, &created.user_id).await?;
        Ok(Reply {
            session_id: created.id,
            user_message_id: created.user_id,
            text,
        })
    }

    async fn create(&self, token: &Secret, prompt: &str) -> Result<Created> {
        let body = serde_json::json!({
            "uuid": uuid::Uuid::now_v7().to_string(),
            "name": "Storage CLI assistance",
            "initial_events": [{"type": "user.message", "contents": [{"type": "text", "text": prompt}]}]
        });
        let mut response = self
            .http
            .post(self.endpoint(&["v1", "sessions"])?)
            .bearer_auth(token.expose())
            .json(&body)
            .send()
            .await
            .map_err(http_error)?;
        check_response(&response, StatusCode::CREATED, "application/json")?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(http_error)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_CREATE_BYTES {
                return Err(SessionError::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        parse_created(&bytes, prompt)
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| SessionError::Configuration)?
            .extend(segments);
        Ok(url)
    }
}

fn transport(is_https: bool, timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(is_https)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .build()
        .map_err(|_| SessionError::Configuration)
}

fn validate_input(token: &Secret, prompt: &str) -> Result<()> {
    let is_token_valid = !token.expose().is_empty()
        && token.expose().len() <= 16 * 1024
        && token.expose().bytes().all(|byte| byte.is_ascii_graphic());
    if !is_token_valid || prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        return Err(SessionError::Input);
    }
    Ok(())
}

fn http_error(error: reqwest::Error) -> SessionError {
    if error.is_timeout() {
        SessionError::Timeout
    } else {
        SessionError::Provider
    }
}

fn check_response(response: &Response, expected: StatusCode, content_type: &str) -> Result<()> {
    // [Review Fix #CLI2] Permission/quota failures need distinct recovery, never a login loop.
    match response.status() {
        StatusCode::UNAUTHORIZED => return Err(SessionError::Unauthorized),
        StatusCode::FORBIDDEN => return Err(SessionError::PermissionDenied),
        StatusCode::TOO_MANY_REQUESTS => return Err(SessionError::RateLimited),
        _ => {}
    }
    if response.status() != expected {
        return Err(SessionError::Provider);
    }
    let media_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next());
    if !media_type.is_some_and(|value| value.trim().eq_ignore_ascii_case(content_type)) {
        return Err(SessionError::Response);
    }
    if content_type == "application/json"
        && response
            .content_length()
            .is_some_and(|length| length > MAX_CREATE_BYTES as u64)
    {
        return Err(SessionError::Response);
    }
    Ok(())
}

#[derive(Deserialize)]
struct Envelope {
    code: i64,
    data: Session,
}
#[derive(Deserialize)]
struct Session {
    id: String,
    status: String,
    messages: Vec<UserMessage>,
}
#[derive(Deserialize)]
struct UserMessage {
    id: String,
    session_id: String,
    role: String,
    status: String,
    items: Vec<InputItem>,
}
#[derive(Deserialize)]
struct InputItem {
    r#type: String,
    text: String,
}
struct Created {
    id: String,
    user_id: String,
}

fn parse_created(bytes: &[u8], prompt: &str) -> Result<Created> {
    let structure: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| SessionError::Response)?;
    validate_create_structure(&structure)?;
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| SessionError::Response)?;
    let session = envelope.data;
    if envelope.code != 0 || session.status != "in_progress" || session.messages.len() != 1 {
        return Err(SessionError::Response);
    }
    valid_id(&session.id)?;
    let user = session
        .messages
        .into_iter()
        .next()
        .ok_or(SessionError::Response)?;
    valid_id(&user.id)?;
    if user.session_id != session.id
        || user.role != "user"
        || user.status != "completed"
        || user.items.len() != 1
        || user.items[0].r#type != "text"
        || user.items[0].text != prompt
    {
        return Err(SessionError::Response);
    }
    Ok(Created {
        id: session.id,
        user_id: user.id,
    })
}

fn validate_create_structure(value: &serde_json::Value) -> Result<()> {
    let messages = value
        .get("data")
        .filter(|inner| inner.is_object())
        .and_then(|inner| inner.get("messages"))
        .and_then(|inner| inner.as_array())
        .ok_or(SessionError::Response)?;
    for message in messages {
        let items = message
            .get("items")
            .and_then(|inner| inner.as_array())
            .ok_or(SessionError::Response)?;
        if !message.is_object() || items.iter().any(|item| !item.is_object()) {
            return Err(SessionError::Response);
        }
    }
    Ok(())
}

fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 256
        || matches!(id, "." | "..")
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !b"/\\%".contains(&byte))
    {
        return Err(SessionError::Response);
    }
    Ok(())
}

#[cfg(test)]
impl SessionClient {
    fn for_test(base: String, timeout: Duration) -> Self {
        let base = Url::parse(&base).unwrap();
        assert_eq!(base.host_str(), Some("127.0.0.1"));
        Self {
            http: transport(false, timeout).unwrap(),
            base,
            timeout,
        }
    }
}
#[cfg(test)]
mod tests;
