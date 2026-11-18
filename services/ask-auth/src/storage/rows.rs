use super::{RefreshRequest, Session, StorageError};
use sqlx::{
    mysql::{MySqlArguments, MySqlRow},
    query::Query,
    MySql, Row,
};
use uuid::Uuid;

// [Review Fix #6] Binary text collations set MySQL BINARY flags; unchecked type compatibility still validates UTF-8 in String decoding.
pub(super) fn session(row: MySqlRow) -> Result<Session, StorageError> {
    Ok(Session {
        auth_id: uuid(row.try_get("auth_id")?)?,
        app_id: row.try_get_unchecked("app_id")?,
        status: row.try_get_unchecked("status")?,
        request_hash: digest(row.try_get("request_hash")?)?,
        login_ticket_hash: digest(row.try_get("login_ticket_hash")?)?,
        claim_challenge: digest(row.try_get("claim_challenge")?)?,
        oauth_state_hash: digest(row.try_get("oauth_state_hash")?)?,
        hmac_key_version: row.try_get("hmac_key_version")?,
        encrypted_oauth_context: row.try_get("encrypted_oauth_context")?,
        claim_request_id: row
            .try_get::<Option<Vec<u8>>, _>("claim_request_id")?
            .map(uuid)
            .transpose()?,
        encrypted_result: row.try_get("encrypted_result")?,
        login_expires_at: row.try_get("login_expires_at")?,
        user_id: row.try_get_unchecked("user_id")?,
        tenant_id: row.try_get_unchecked("tenant_id")?,
        scope_json: row
            .try_get::<Option<sqlx::types::Json<Vec<String>>>, _>("scope_json")?
            .map(|value| value.0),
        broker_hash: digest(row.try_get("broker_hash")?)?,
        refresh_token_hash: digest(row.try_get("refresh_token_hash")?)?,
        credential_generation: row.try_get("credential_generation")?,
        auth_expires_at: row.try_get("auth_expires_at")?,
        reason_code: row.try_get_unchecked("reason_code")?,
    })
}
pub(super) fn refresh(row: MySqlRow) -> Result<RefreshRequest, StorageError> {
    Ok(RefreshRequest {
        auth_id: uuid(row.try_get("auth_id")?)?,
        request_id: uuid(row.try_get("request_id")?)?,
        request_hash: row
            .try_get::<Vec<u8>, _>("request_hash")?
            .try_into()
            .map_err(|_| StorageError::Integrity)?,
        hmac_key_version: row.try_get("hmac_key_version")?,
        input_generation: row.try_get("input_generation")?,
        status: row.try_get_unchecked("status")?,
        expires_at: row.try_get("expires_at")?,
        encrypted_response: row.try_get("encrypted_response")?,
        error_code: row.try_get_unchecked("error_code")?,
    })
}
fn uuid(bytes: Vec<u8>) -> Result<Uuid, StorageError> {
    Uuid::from_slice(&bytes).map_err(|_| StorageError::Integrity)
}
fn digest(bytes: Option<Vec<u8>>) -> Result<Option<[u8; 32]>, StorageError> {
    bytes
        .map(|value| value.try_into().map_err(|_| StorageError::Integrity))
        .transpose()
}
pub(super) fn bind_session<'q>(
    query: Query<'q, MySql, MySqlArguments>,
    row: &'q Session,
) -> Query<'q, MySql, MySqlArguments> {
    query
        .bind(&row.app_id)
        .bind(&row.status)
        .bind(row.request_hash.map(Vec::from))
        .bind(row.login_ticket_hash.map(Vec::from))
        .bind(row.claim_challenge.map(Vec::from))
        .bind(row.oauth_state_hash.map(Vec::from))
        .bind(row.hmac_key_version)
        .bind(&row.encrypted_oauth_context)
        .bind(row.claim_request_id.map(|id| id.as_bytes().to_vec()))
        .bind(&row.encrypted_result)
        .bind(row.login_expires_at)
        .bind(&row.user_id)
        .bind(&row.tenant_id)
        .bind(row.scope_json.as_ref().map(sqlx::types::Json))
        .bind(row.broker_hash.map(Vec::from))
        .bind(row.refresh_token_hash.map(Vec::from))
        .bind(row.credential_generation)
        .bind(row.auth_expires_at)
        .bind(&row.reason_code)
        .bind(row.auth_id.as_bytes().as_slice())
}
pub(super) const INSERT_SESSION: &str = "INSERT INTO auth_sessions (app_id,status,request_hash,login_ticket_hash,claim_challenge,oauth_state_hash,hmac_key_version,encrypted_oauth_context,claim_request_id,encrypted_result,login_expires_at,user_id,tenant_id,scope_json,broker_hash,refresh_token_hash,credential_generation,auth_expires_at,reason_code,auth_id) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)";
pub(super) const UPDATE_SESSION: &str = "UPDATE auth_sessions SET app_id=?,status=?,request_hash=?,login_ticket_hash=?,claim_challenge=?,oauth_state_hash=?,hmac_key_version=?,encrypted_oauth_context=?,claim_request_id=?,encrypted_result=?,login_expires_at=?,user_id=?,tenant_id=?,scope_json=?,broker_hash=?,refresh_token_hash=?,credential_generation=?,auth_expires_at=?,reason_code=? WHERE auth_id=?";
pub(super) fn bind_refresh<'q>(
    query: Query<'q, MySql, MySqlArguments>,
    row: &'q RefreshRequest,
) -> Query<'q, MySql, MySqlArguments> {
    query
        .bind(row.request_hash.as_slice())
        .bind(row.hmac_key_version)
        .bind(row.input_generation)
        .bind(&row.status)
        .bind(row.expires_at)
        .bind(&row.encrypted_response)
        .bind(&row.error_code)
        .bind(row.auth_id.as_bytes().as_slice())
        .bind(row.request_id.as_bytes().as_slice())
}
pub(super) const INSERT_REFRESH: &str = "INSERT INTO auth_refresh_requests (request_hash,hmac_key_version,input_generation,status,expires_at,encrypted_response,error_code,auth_id,request_id) VALUES (?,?,?,?,?,?,?,?,?)";
pub(super) const UPDATE_REFRESH: &str = "UPDATE auth_refresh_requests SET request_hash=?,hmac_key_version=?,input_generation=?,status=?,expires_at=?,encrypted_response=?,error_code=? WHERE auth_id=? AND request_id=?";
