use super::{Config, Error, Result, Secret};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use zeroize::Zeroizing;

const AUTHORIZE: &str = "https://accounts.doubao.com/open-apis/authen/v1/authorize";
const MAX_REQUEST: usize = 8192;

pub(crate) struct Parameters {
    pub(crate) url: Zeroizing<String>,
    pub(crate) state: Secret,
    pub(crate) verifier: Secret,
}
/// A single-use callback listener. Dropping it closes the socket without background tasks.
pub struct PendingLogin {
    listener: TcpListener,
    parameters: Parameters,
    redirect_uri: String,
}
/// A validated authorization code and its original PKCE binding; cannot be reused by value.
pub struct AuthorizationCode {
    pub(crate) code: Secret,
    pub(crate) verifier: Secret,
    pub(crate) redirect_uri: String,
}

pub(crate) fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
fn random_secret() -> Result<Secret> {
    let mut bytes = zeroize::Zeroizing::new([0u8; 32]);
    OsRng
        .try_fill_bytes(bytes.as_mut())
        .map_err(|_| Error::Configuration)?;
    Ok(Secret::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}
pub(crate) fn authorization_parameters(config: &Config) -> Result<Parameters> {
    let state = random_secret()?;
    let verifier = random_secret()?;
    let mut url = url::Url::parse(AUTHORIZE).map_err(|_| Error::Configuration)?;
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", &config.app_id),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("scope", &config.required_scopes.join(" ")),
        ("state", state.expose()),
        ("code_challenge", &challenge(verifier.expose())),
        ("code_challenge_method", "S256"),
    ]);
    Ok(Parameters {
        url: Zeroizing::new(url.into()),
        state,
        verifier,
    })
}
impl PendingLogin {
    pub(crate) async fn bind(config: &Config) -> Result<Self> {
        let ip = match config.redirect_uri.host_str() {
            Some("[::1]") => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            _ => IpAddr::V4(Ipv4Addr::LOCALHOST),
        };
        let address = SocketAddr::new(ip, config.redirect_uri.port().ok_or(Error::Configuration)?);
        let listener = TcpListener::bind(address)
            .await
            .map_err(|_| Error::Callback)?;
        Ok(Self {
            listener,
            parameters: authorization_parameters(config)?,
            redirect_uri: config.redirect_uri.to_string(),
        })
    }
    /// Authorization URL to display or open in the user's browser after binding the listener.
    pub fn authorization_url(&self) -> &str {
        &self.parameters.url
    }
    /// Wait once for a bounded, authenticated callback. Maximum allowed wait is ten minutes.
    /// Any denied, malformed or unauthenticated callback fails closed and closes the listener.
    pub async fn wait(self, timeout: Duration) -> Result<AuthorizationCode> {
        if timeout.is_zero() || timeout > Duration::from_secs(600) {
            return Err(Error::Configuration);
        }
        let code = tokio::time::timeout(timeout, self.receive())
            .await
            .map_err(|_| Error::Timeout)??;
        Ok(AuthorizationCode {
            code,
            verifier: self.parameters.verifier,
            redirect_uri: self.redirect_uri,
        })
    }
    async fn receive(&self) -> Result<Secret> {
        let (mut stream, peer) = self.listener.accept().await.map_err(|_| Error::Callback)?;
        if !peer.ip().is_loopback() {
            return Err(Error::Callback);
        }
        let request = read_request(&mut stream).await?;
        let code = parse_callback(&request, self.parameters.state.expose());
        let response = if code.is_ok() {
            "HTTP/1.1 200 OK\r\nContent-Length: 34\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\nLogin received. Close this window."
        } else {
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 18\r\nConnection: close\r\n\r\nCallback rejected."
        };
        stream
            .write_all(response.as_bytes())
            .await
            .map_err(|_| Error::Callback)?;
        stream.shutdown().await.map_err(|_| Error::Callback)?;
        code
    }
}
async fn read_request(stream: &mut TcpStream) -> Result<Zeroizing<String>> {
    // [Review Fix #17] All application-owned callback bytes and text zeroize on every exit path.
    let mut bytes = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 1024]);
    loop {
        let count = stream
            .read(chunk.as_mut())
            .await
            .map_err(|_| Error::Callback)?;
        if count == 0 {
            return Err(Error::Callback);
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_REQUEST {
            return Err(Error::Callback);
        }
        if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::Callback)?;
    Ok(Zeroizing::new(text.to_owned()))
}
// Secret-owned map keys preserve duplicate detection without ordinary decoded-string copies.
struct CallbackKey(Secret);
impl std::borrow::Borrow<str> for CallbackKey {
    fn borrow(&self) -> &str {
        self.0.expose()
    }
}
impl PartialEq for CallbackKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose() == other.0.expose()
    }
}
impl Eq for CallbackKey {}
impl std::hash::Hash for CallbackKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(self.0.expose(), state);
    }
}
pub(crate) fn parse_callback(request: &str, expected: &str) -> Result<Secret> {
    let mut words = request
        .lines()
        .next()
        .ok_or(Error::Callback)?
        .split_whitespace();
    if words.next() != Some("GET") {
        return Err(Error::Callback);
    }
    let target = words.next().ok_or(Error::Callback)?;
    if words.next() != Some("HTTP/1.1") || words.next().is_some() {
        return Err(Error::Callback);
    }
    let (path, query) = target.split_once('?').ok_or(Error::Callback)?;
    if path != "/callback" || query.len() > MAX_REQUEST {
        return Err(Error::Callback);
    }
    let mut parameters: HashMap<CallbackKey, Secret> = HashMap::new();
    // [Review Fix #12] Form parsing is intentionally strict: invalid escapes/UTF-8 must not be replaced.
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or(Error::Callback)?;
        let key = decode_component(key)?;
        let value = decode_component(value)?;
        if parameters.insert(CallbackKey(key), value).is_some() {
            return Err(Error::Callback);
        }
    }
    let state = parameters.get("state").ok_or(Error::Callback)?;
    if !bool::from(state.expose().as_bytes().ct_eq(expected.as_bytes())) {
        return Err(Error::Callback);
    }
    if parameters.contains_key("error") {
        return Err(Error::Denied);
    }
    // [Review Fix #13] Authorization codes must be bounded visible ASCII after strict decoding.
    let code = parameters
        .remove("code")
        .filter(|v| {
            !v.expose().is_empty()
                && v.expose().len() <= 4096
                && v.expose().bytes().all(|byte| byte.is_ascii_graphic())
        })
        .ok_or(Error::Callback)?;
    Ok(code)
}

fn decode_component(value: &str) -> Result<Secret> {
    let mut decoded = zeroize::Zeroizing::new(Vec::new());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'%' => {
                let high = bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or(Error::Callback)?;
                let low = bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or(Error::Callback)?;
                decoded.push((high * 16 + low) as u8);
            }
            b'+' => decoded.push(b' '),
            _ => decoded.push(byte),
        }
    }
    // [Review Fix #18] Transfer decoded values directly into secret owners, including keys.
    Ok(Secret::new(
        std::str::from_utf8(&decoded)
            .map_err(|_| Error::Callback)?
            .to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ephemeral_config() -> Config {
        let mut config = Config::new(
            "test-app".into(),
            "http://127.0.0.1:8080/callback",
            vec!["test:read".into()],
            "/private/tmp/ask-callback-fixture".into(),
        )
        .unwrap();
        config.redirect_uri.set_port(Some(0)).unwrap();
        config
    }
    #[tokio::test]
    async fn real_callback_success_and_denial_close_listener() {
        for outcome in [
            "code=fixture-code",
            "error=access_denied",
            "code=a&code=b",
            "code=too-long",
        ] {
            let is_denied = outcome == "error=access_denied";
            let is_invalid = outcome == "code=a&code=b" || outcome == "code=too-long";
            let config = ephemeral_config();
            let pending = PendingLogin::bind(&config).await.unwrap();
            let address = pending.listener.local_addr().unwrap();
            let state = url::Url::parse(pending.authorization_url())
                .unwrap()
                .query_pairs()
                .find(|(key, _)| key == "state")
                .unwrap()
                .1
                .into_owned();
            let waiting = tokio::spawn(pending.wait(Duration::from_secs(2)));
            let mut stream = TcpStream::connect(address).await.unwrap();
            let outcome = if outcome == "code=too-long" {
                format!("code={}", "x".repeat(9000))
            } else {
                outcome.into()
            };
            let request = format!(
                "GET /callback?state={state}&{outcome} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut response = String::new();
            let read = stream.read_to_string(&mut response).await;
            if !is_invalid {
                read.unwrap();
            }
            let result = waiting.await.unwrap();
            if is_invalid {
                assert!(result.is_err());
            } else if is_denied {
                assert!(matches!(result, Err(Error::Denied)));
                assert!(response.starts_with("HTTP/1.1 400"));
            } else {
                assert_eq!(result.unwrap().code.expose(), "fixture-code");
                assert!(response.starts_with("HTTP/1.1 200"));
            }
            assert!(TcpStream::connect(address).await.is_err());
        }
    }
}

#[cfg(test)]
mod sensitive_owner_tests {
    use super::*;
    #[tokio::test]
    async fn callback_decoded_and_raw_values_have_zeroizing_owners() {
        let decoded: Secret = decode_component("fixture%2Fcode").unwrap();
        assert_eq!(decoded.expose(), "fixture/code");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let sender = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream
                .write_all(b"GET /callback?code=fixture HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        let raw: zeroize::Zeroizing<String> = read_request(&mut stream).await.unwrap();
        assert!(raw.contains("code=fixture"));
        sender.await.unwrap();
    }
}
