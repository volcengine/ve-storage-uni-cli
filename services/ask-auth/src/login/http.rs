use super::{LoginError, LoginService};
use crate::{
    http::callback_response,
    validation::{self, Validate},
};
use axum::{
    body::to_bytes,
    extract::{MatchedPath, Request},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;

const MAX_BODY_BYTES: usize = 32 * 1024;
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(5);

impl LoginService {
    pub(crate) async fn handle(&self, request: Request, request_id: &str) -> Response {
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map(|path| path.as_str())
            .unwrap_or("");
        if route == "/v1/oauth/doubao/callback" {
            let result = self.callback(request.uri().query()).await;
            return callback_response(match result {
                Ok(()) => StatusCode::OK,
                Err(LoginError::LoginExpired) => StatusCode::GONE,
                Err(LoginError::InvalidRequest | LoginError::AuthenticationRequired) => {
                    StatusCode::BAD_REQUEST
                }
                Err(_) => StatusCode::SERVICE_UNAVAILABLE,
            });
        }
        if route == "/readyz" {
            return self.readiness().await;
        }
        let success = if route == "/v1/auth/login-attempts" {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        match self.business(request).await {
            Ok(payload) => (
                success,
                Json(json!({"schema_version":1,"request_id":request_id,"data":payload})),
            )
                .into_response(),
            Err(error) => error.response(request_id),
        }
    }
    async fn readiness(&self) -> Response {
        // [Review Fix #32] Sample DB now after policy loading so a newly fetched policy is not compared with an older clock.
        let is_ready = match self.policy().await {
            Ok(policy) => self
                .store
                .login_snapshot(uuid::Uuid::now_v7())
                .await
                .is_ok_and(|(_, now)| policy.is_fresh(now)),
            Err(_) => false,
        };
        (
            if is_ready {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            Json(json!({"status":if is_ready { "ok" } else { "unavailable" }})),
        )
            .into_response()
    }
    async fn business(&self, request: Request) -> Result<Value, LoginError> {
        if request.uri().path() == "/v1/auth/status" {
            validation::validate_status(request.uri().query())?;
            let fields = validation::query_fields(request.uri().query())?;
            let auth_id = parse_id(fields.get("auth_id").ok_or(LoginError::InvalidRequest)?)?;
            return self.status(auth_id, bearer(request.headers())?).await;
        }
        if !validation::query_fields(request.uri().query())?.is_empty() {
            return Err(LoginError::InvalidRequest);
        }
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map(|path| path.as_str())
            .unwrap_or("");
        match route {
            "/v1/auth/login-attempts" => self.create(read_json(request).await?).await,
            "/v1/auth/login-attempts/{auth_id}" => {
                let auth_id = request
                    .uri()
                    .path()
                    .rsplit('/')
                    .next()
                    .filter(|id| validation::is_auth_id(id))
                    .ok_or(LoginError::InvalidRequest)?;
                self.poll(parse_id(auth_id)?, bearer(request.headers())?)
                    .await
            }
            "/v1/auth/token" => {
                let ticket = bearer(request.headers())?.to_owned();
                let key = single_header(request.headers(), "idempotency-key")
                    .filter(|id| validation::is_request_id(id))
                    .ok_or(LoginError::InvalidRequest)?;
                let key = parse_id(key)?;
                self.claim(read_json(request).await?, &ticket, key).await
            }
            "/v1/auth/refresh" => {
                let broker = zeroize::Zeroizing::new(bearer(request.headers())?.to_owned());
                let key = single_header(request.headers(), "idempotency-key")
                    .filter(|id| validation::is_request_id(id))
                    .ok_or(LoginError::InvalidRequest)?;
                let key = parse_id(key)?;
                self.refresh(read_json(request).await?, &broker, key).await
            }
            "/v1/auth/logout" => {
                let broker = zeroize::Zeroizing::new(bearer(request.headers())?.to_owned());
                self.logout(read_json(request).await?, &broker).await
            }
            _ => Err(LoginError::Unavailable),
        }
    }
}
fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(first)
}
fn bearer(headers: &HeaderMap) -> Result<&str, LoginError> {
    single_header(headers, "authorization")
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, ticket)| ticket)
        .filter(|ticket| validation::is_nonce(ticket))
        .ok_or(LoginError::AuthenticationRequired)
}
async fn read_json<T: DeserializeOwned + Validate>(request: Request) -> Result<T, LoginError> {
    let is_json = single_header(request.headers(), "content-type")
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return Err(LoginError::InvalidRequest);
    }
    let bytes = tokio::time::timeout(
        BODY_READ_TIMEOUT,
        to_bytes(request.into_body(), MAX_BODY_BYTES),
    )
    .await
    .map_err(|_| LoginError::InvalidRequest)?
    .map_err(|_| LoginError::InvalidRequest)?;
    // [Review Fix #71] Frozen POST schemas require objects; reject Serde positional arrays before typed parsing.
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(LoginError::InvalidRequest);
    }
    crate::json::check_json_depth(&bytes).map_err(|_| LoginError::InvalidRequest)?;
    let payload: T = serde_json::from_slice(&bytes).map_err(|_| LoginError::InvalidRequest)?;
    payload.validate()?;
    Ok(payload)
}
pub(super) fn parse_id(value: &str) -> Result<uuid::Uuid, LoginError> {
    uuid::Uuid::parse_str(value).map_err(|_| LoginError::InvalidRequest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::LoginCreateRequest;
    use axum::body::Body;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    fn create_fields() -> Value {
        let nonce = URL_SAFE_NO_PAD.encode([1; 32]);
        json!({
            "auth_id": uuid::Uuid::now_v7().to_string(),
            "login_ticket": nonce,
            "claim_challenge": nonce,
            "device_name": "workstation",
            "cli_version": "0.1.0",
        })
    }
    fn request(body: String) -> Request {
        Request::builder()
            .method("POST")
            .uri("/v1/auth/login-attempts")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn typed_create_body_requires_an_object_instead_of_a_positional_array() {
        let fields = create_fields();
        let positional: Vec<Value> = [
            "auth_id",
            "login_ticket",
            "claim_challenge",
            "device_name",
            "cli_version",
        ]
        .iter()
        .map(|field| fields[field].clone())
        .collect();
        let response =
            read_json::<LoginCreateRequest>(request(json!(positional).to_string())).await;
        assert!(matches!(response, Err(LoginError::InvalidRequest)));
    }

    #[tokio::test]
    async fn typed_create_body_keeps_object_and_duplicate_field_validation() {
        let mut fields = create_fields();
        fields["device_name"] = json!(r#"work [station] {"quoted"}"#);
        let accepted = read_json::<LoginCreateRequest>(request(format!(" \r\n\t{}", fields)))
            .await
            .unwrap();
        assert_eq!(accepted.auth_id, fields["auth_id"].as_str().unwrap());
        let duplicate = fields
            .to_string()
            .replacen("{", "{\"device_name\":\"other\",", 1);
        assert!(matches!(
            read_json::<LoginCreateRequest>(request(duplicate)).await,
            Err(LoginError::InvalidRequest)
        ));
    }
}
