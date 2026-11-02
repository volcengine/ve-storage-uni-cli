//! Explicit OAuth receiver construction and isolated receipt regressions.

use super::common::{build_adrive_runtime, map_ids_error};
use crate::domain::{share_access::ShareClient, share_url::ParsedShareUrl};
use serde::{Deserialize, Serialize};
use tos_core::agent::{error::CliError, global_args::GlobalArgs};

/// Non-secret identity bound into authenticated download recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiverContext {
    pub api_origin: String,
    pub profile: String,
    pub user_id: Option<String>,
}

/// Load only the explicitly selected OAuth profile and build the receiver API client.
/// Uses configured/overridden API service rather than sending credentials to a copied page origin.
/// Returns existing profile/login/binding errors without initiating an interactive login.
pub(crate) fn oauth_receiver(
    global: &GlobalArgs,
    identity: &ParsedShareUrl,
) -> Result<(ShareClient, ReceiverContext), CliError> {
    let runtime = build_adrive_runtime(global, Some(crate::domain::auth::AuthMode::Oauth))?;
    let client = runtime
        .client
        .receiver_share_client(&identity.origin)
        .map_err(map_ids_error)?
        .with_request_trace(global.request_trace.clone())
        .with_redactions(identity.password.iter().cloned().collect());
    let context = ReceiverContext {
        api_origin: client.api_origin(),
        profile: global.profile.clone(),
        user_id: None,
    };
    Ok((client, context))
}

#[cfg(test)]
mod tests {
    use crate::cli::{share::*, ADriveAuthArgs};
    use crate::domain::auth::AuthMode;
    use crate::domain::share_access::fixture::{reply, Fixture};
    use tos_core::agent::{global_args::GlobalArgs, output::OutputFormat};
    use tos_core::infra::credentials::{CredentialsFile, StoredOAuthCredentials};

    fn oauth_config(endpoint: &str) -> (GlobalArgs, std::path::PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("share-receiver-oauth-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("credentials.toml");
        std::fs::write(&config_path, format!("[receiver.adrive]\nauth_mode='oauth'\nendpoint='{endpoint}'\nregion='cn-beijing'\nmax_retry_count=0\n")).unwrap();
        let mut credentials = CredentialsFile::default();
        credentials
            .set_adrive_oauth(
                "receiver",
                StoredOAuthCredentials {
                    access_token: Some("receiver-oauth-token".into()),
                    expires_at: Some(
                        (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                    ),
                    instance_id: Some("inst".into()),
                    user_id: Some("receiver-user".into()),
                    auth_endpoint: Some("https://auth.example.test".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        credentials.save_to_path(&credentials_path).unwrap();
        (
            GlobalArgs {
                config_path: Some(config_path),
                credentials_path: Some(credentials_path),
                profile: "receiver".into(),
                output: Some(OutputFormat::Json),
                ..Default::default()
            },
            directory,
        )
    }

    fn files_action(share_url: String) -> ShareCommand {
        ShareCommand {
            action: Some(ShareAction::Files(ShareFilesArgs {
                share_url,
                path: Some("docs/".into()),
                marker: Some("previous-page".into()),
                max_keys: 2,
                human_readable: false,
                sort: None,
                columns: None,
                manifest_path: None,
                is_password_stdin: false,
            })),
        }
    }

    fn download_action(endpoint: &str, directory: &std::path::Path) -> ShareCommand {
        ShareCommand {
            action: Some(ShareAction::Download(ShareDownloadArgs {
                share_url: format!("{endpoint}/share/s?pwd=receiver-password"),
                destination_dir: directory.join("output").to_string_lossy().into(),
                path: Some("a.txt".into()),
                is_recursive: false,
                is_overwrite: false,
                is_password_stdin: false,
                checkpoint: Some(directory.join("journal.json").to_string_lossy().into()),
                report_path: Some(directory.join("report.csv").to_string_lossy().into()),
                manifest_path: Some(directory.join("manifest.csv").to_string_lossy().into()),
                is_resume: false,
                checkpoint_threshold: None,
                multipart_concurrency: None,
                is_progress: false,
                is_no_progress: false,
                ..Default::default()
            })),
        }
    }

    fn download_fixture(content_endpoint: &str) -> Fixture {
        Fixture::new(vec![
            reply(
                200,
                r#"{"ShareID":"s","RequireLogin":true,"HasPassword":true}"#,
            ),
            reply(
                200,
                r#"{"AccessToken":"receiver-share-token","ExpiresIn":60}"#,
            ),
            reply(200, r#"{"FilePath":"a.txt","Size":3,"ETag":"abc"}"#),
            reply(
                200,
                &format!(
                    r#"{{"DownloadURL":"{}/content?signature=content-secret"}}"#,
                    content_endpoint
                ),
            ),
        ])
    }

    fn assert_checkpoint_recipient(journal: &str) {
        for secret in [
            "receiver-oauth-token",
            "receiver-password",
            "receiver-share-token",
            "content-secret",
        ] {
            assert!(!journal.contains(secret));
        }
        let identity: serde_json::Value = serde_json::from_str(journal).unwrap();
        assert_eq!(identity["identity"]["recipient"]["profile"], "receiver");
        assert_eq!(
            identity["identity"]["recipient"]["user_id"],
            "receiver-user"
        );
    }

    #[tokio::test]
    async fn oauth_download_keeps_content_and_checkpoint_free_of_credentials() {
        let content =
            Fixture::new(vec![reply(200, "abc")
                .replace("Connection: close", "ETag: \"abc\"\r\nConnection: close")]);
        let api = download_fixture(&content.endpoint);
        let (global, directory) = oauth_config(&api.endpoint);
        let result = super::super::share::handle_share_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            &download_action(&api.endpoint, &directory),
        )
        .await;
        let requests = api.finish();
        let transfers = content.finish();
        let journal = std::fs::read_to_string(directory.join("journal.json")).unwrap();
        assert_eq!(
            std::fs::read(directory.join("output/a.txt")).unwrap(),
            b"abc"
        );
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(result.unwrap(), 0);
        assert_eq!(requests.len(), 4);
        assert!(requests[1].contains("receiver-password"));
        for request in &requests[2..] {
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            assert!(request.contains("x-share-token: receiver-share-token"));
        }
        assert_eq!(transfers.len(), 1);
        assert!(!transfers[0].to_ascii_lowercase().contains("authorization:"));
        assert!(!transfers[0].to_ascii_lowercase().contains("x-share-token:"));
        assert_checkpoint_recipient(&journal);
    }

    #[tokio::test]
    async fn oauth_token_exchange_denial_has_no_identity_or_password_fallback() {
        let api = Fixture::new(vec![
            reply(
                200,
                r#"{"ShareID":"s","RequireLogin":true,"HasPassword":true}"#,
            ),
            reply(403, r#"{"Code":"Forbidden.SharePassword"}"#),
        ]);
        let (global, directory) = oauth_config(&api.endpoint);
        let result = super::super::share::handle_share_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            &files_action(format!("{}/share/s?pwd=receiver-password", api.endpoint)),
        )
        .await;
        let requests = api.finish();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Forbidden.SharePassword"));
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("receiver-password"));
    }

    #[test]
    fn omitted_mode_does_not_read_invalid_account_files() {
        let (mut global, directory) = oauth_config("http://127.0.0.1:1");
        std::fs::write(
            global.config_path.as_ref().unwrap(),
            "invalid account configuration",
        )
        .unwrap();
        std::fs::write(
            global.credentials_path.as_ref().unwrap(),
            "invalid credentials",
        )
        .unwrap();
        global.profile = "absent-profile".into();
        for mode in [None, Some(AuthMode::Aksk), Some(AuthMode::Unified)] {
            assert!(super::super::share_receive::ShareReceiver::for_command(
                &global,
                &ADriveAuthArgs { auth_mode: mode },
                "http://127.0.0.1:1/share/s"
            )
            .is_ok());
        }
        assert!(super::super::share_receive::ShareReceiver::for_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth)
            },
            "http://127.0.0.1:1/share/s"
        )
        .is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn explicit_oauth_receives_login_required_share_and_detaches_file_auth() {
        let fixture = Fixture::new(vec![
            reply(200, r#"{"ShareID":"s","RequireLogin":true}"#),
            reply(
                200,
                r#"{"AccessToken":"receiver-share-token","ExpiresIn":60}"#,
            ),
            reply(200, r#"{"Files":[],"Folders":[],"IsTruncated":false}"#),
        ]);
        let (global, directory) = oauth_config(&fixture.endpoint);
        let result = super::super::share::handle_share_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            &files_action(format!("{}/share/s", fixture.endpoint)),
        )
        .await;
        let requests = fixture.finish();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(result.unwrap(), 0);
        assert_eq!(requests.len(), 3);
        for request in &requests[..2] {
            assert!(request.contains("authorization: Bearer receiver-oauth-token"));
        }
        assert!(!requests[2].to_ascii_lowercase().contains("authorization:"));
        assert!(requests[2].contains("x-share-token: receiver-share-token"));
        assert!(requests[2].contains("limit=2") && requests[2].contains("marker=previous-page"));
    }

    #[tokio::test]
    async fn explicit_oauth_inspect_sends_recipient_identity() {
        let fixture = Fixture::new(vec![reply(200, r#"{"ShareID":"s","RequireLogin":true}"#)]);
        let (global, directory) = oauth_config(&fixture.endpoint);
        let command = ShareCommand {
            action: Some(ShareAction::Inspect(InspectShareArgs {
                share_url: format!("{}/share/s", fixture.endpoint),
            })),
        };
        let result = super::super::share::handle_share_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            &command,
        )
        .await;
        let requests = fixture.finish();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(result.unwrap(), 0);
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("authorization: Bearer receiver-oauth-token"));
    }

    #[tokio::test]
    async fn oauth_receipt_redacts_account_token_echoed_by_file_metadata() {
        let fixture = Fixture::new(vec![
            reply(200, r#"{"ShareID":"s","RequireLogin":true}"#),
            reply(
                200,
                r#"{"AccessToken":"receiver-share-token","ExpiresIn":60}"#,
            ),
            reply(200, r#"{"Files":[],"Folders":[]}"#)
                .replace("fixture-id", "receiver-oauth-token"),
        ]);
        let (global, directory) = oauth_config(&fixture.endpoint);
        let result = super::super::share::handle_share_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            &files_action(format!("{}/share/s", fixture.endpoint)),
        )
        .await;
        fixture.finish();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(result.unwrap(), 0);
        assert!(!format!("{:?}", global.request_trace.snapshot()).contains("receiver-oauth-token"));
    }

    #[tokio::test]
    async fn omitted_mode_ignores_profile_oauth_and_never_falls_back() {
        let fixture = Fixture::new(vec![reply(200, r#"{"ShareID":"s","RequireLogin":true}"#)]);
        let (global, directory) = oauth_config(&fixture.endpoint);
        let result = super::super::share::handle_share_command(
            &global,
            &ADriveAuthArgs::default(),
            &files_action(format!("{}/share/s", fixture.endpoint)),
        )
        .await;
        let requests = fixture.finish();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("share_login_required"));
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].to_ascii_lowercase().contains("authorization:"));
    }

    #[tokio::test]
    async fn oauth_uses_configured_api_instead_of_copied_page_origin() {
        let fixture = Fixture::new(vec![reply(200, r#"{"ShareID":"s"}"#)]);
        let (global, directory) = oauth_config(&fixture.endpoint);
        let receiver = super::super::share_receive::ShareReceiver::for_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            "http://127.0.0.1:1/share/s",
        )
        .unwrap();
        assert!(receiver.info().await.is_ok());
        let requests = fixture.finish();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("authorization: Bearer receiver-oauth-token"));
    }

    #[test]
    fn wrong_standard_instance_is_rejected_before_network_io() {
        let (global, directory) = oauth_config("http://127.0.0.1:1");
        let result = super::super::share_receive::ShareReceiver::for_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            "http://other.idsapp-boe.volces.com/share/s",
        );
        std::fs::remove_dir_all(directory).unwrap();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("different ADrive Instance"));
    }

    fn with_refresh_credentials(global: &GlobalArgs, endpoint: &str) {
        let path = global.credentials_path.as_ref().unwrap();
        let mut credentials = CredentialsFile::load_from(path).unwrap();
        let mut stored = credentials.adrive_oauth("receiver", path).unwrap();
        stored.refresh_token = Some("receiver-refresh-token".into());
        stored.auth_endpoint = Some(endpoint.into());
        credentials.set_adrive_oauth("receiver", stored).unwrap();
        credentials.save_to_path(path).unwrap();
    }

    #[tokio::test]
    async fn share_token_renewal_rejects_a_changed_recipient_subject() {
        let api = Fixture::new(vec![
            reply(200, r#"{"ShareID":"s","RequireLogin":true}"#),
            reply(200, r#"{"AccessToken":"first-share-token","ExpiresIn":60}"#),
            reply(
                200,
                r#"{"AccessToken":"second-share-token","ExpiresIn":60}"#,
            ),
        ]);
        let (global, directory) = oauth_config(&api.endpoint);
        let mut receiver = super::super::share_receive::ShareReceiver::for_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            &format!("{}/share/s", api.endpoint),
        )
        .unwrap();
        let info = receiver.info().await.unwrap();
        receiver.authorize(&global, &info, false).await.unwrap();
        let path = global.credentials_path.as_ref().unwrap();
        let mut credentials = CredentialsFile::load_from(path).unwrap();
        let mut stored = credentials.adrive_oauth("receiver", path).unwrap();
        stored.user_id = Some("replacement-user".into());
        stored.access_token = Some("replacement-account-token".into());
        credentials.set_adrive_oauth("receiver", stored).unwrap();
        credentials.save_to_path(path).unwrap();
        receiver.expires_at = Some(std::time::Instant::now());
        let result = receiver.ensure_session().await;
        let requests = api.finish();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(
            result.is_err(),
            "recipient must remain bound after initial exchange"
        );
        assert_eq!(requests.len(), 3);
    }

    #[tokio::test]
    async fn oauth_receipt_refreshes_once_and_redacts_both_account_tokens() {
        let auth = Fixture::new(vec![reply(
            200,
            r#"{"access_token":"new-oauth-token","refresh_token":"new-refresh-token","token_type":"Bearer","expires_in":3600,"scope":"file:read file:list","instance_id":"inst"}"#,
        )]);
        let api = Fixture::new(vec![
            reply(401, r#"{"Code":"Unauthorized"}"#),
            reply(200, r#"{"ShareID":"s","RequireLogin":true}"#),
            reply(
                200,
                r#"{"AccessToken":"receiver-share-token","ExpiresIn":60}"#,
            ),
            reply(200, r#"{"Files":[],"Folders":[]}"#)
                .replace("fixture-id", "receiver-oauth-token"),
        ]);
        let (global, directory) = oauth_config(&api.endpoint);
        with_refresh_credentials(&global, &auth.endpoint);
        let result = super::super::share::handle_share_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            &files_action(format!("{}/share/s", api.endpoint)),
        )
        .await;
        let requests = api.finish();
        let refreshes = auth.finish();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(result.unwrap(), 0);
        assert_eq!(refreshes.len(), 1);
        assert_eq!(requests.len(), 4);
        assert!(requests[0].contains("authorization: Bearer receiver-oauth-token"));
        assert!(requests[1].contains("authorization: Bearer new-oauth-token"));
        assert!(requests[2].contains("authorization: Bearer new-oauth-token"));
        assert!(!requests[3].to_ascii_lowercase().contains("authorization:"));
        let trace = format!("{:?}", global.request_trace.snapshot());
        assert!(!trace.contains("receiver-oauth-token") && !trace.contains("new-oauth-token"));
    }
}
