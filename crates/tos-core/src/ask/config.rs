use super::{application, application_values, Error, Result, Secret};
pub(crate) use application_values::valid_scope_token;
use std::path::PathBuf;
use url::Url;

/// Application binding and dedicated local state; deliberately lacks Debug and serialization.
/// Compiled secret bytes remain in the binary; only owned copies are zeroized on drop.
#[derive(Clone)]
pub struct Config {
    pub(crate) app_id: String,
    pub(crate) redirect_uri: Url,
    pub(crate) required_scopes: Vec<String>,
    pub(crate) home: PathBuf,
    pub(crate) credentials_path: PathBuf,
    pub(crate) profile: String,
    pub(crate) secret_file: Option<PathBuf>,
    pub(super) bundled_secret: Option<&'static str>,
}
impl Config {
    /// Validate an app, fixed loopback callback, required scopes and absolute state directory.
    /// Rejects unsafe callbacks, empty business scopes, or ambiguous identifiers.
    pub fn new(
        app_id: String,
        redirect_uri: &str,
        mut required_scopes: Vec<String>,
        home: PathBuf,
    ) -> Result<Self> {
        let redirect_uri = validate_redirect(redirect_uri)?;
        if !application_values::valid_app_id(&app_id) || !home.is_absolute() {
            return Err(Error::Configuration);
        }
        // [Review Fix #4] OAuth scopes use scope-token syntax and cannot contain duplicates.
        if required_scopes.is_empty()
            || required_scopes
                .iter()
                .any(|scope| !valid_scope_token(scope))
        {
            return Err(Error::Configuration);
        }
        let count = required_scopes.len();
        required_scopes.sort();
        required_scopes.dedup();
        if count != required_scopes.len() {
            return Err(Error::Configuration);
        }
        if !required_scopes
            .iter()
            .any(|scope| scope == "offline_access")
        {
            required_scopes.push("offline_access".into());
        }
        required_scopes.sort();
        required_scopes.dedup();
        Ok(Self {
            app_id,
            redirect_uri,
            required_scopes,
            credentials_path: home.join("credentials.toml"),
            profile: "default".into(),
            home,
            secret_file: None,
            bundled_secret: None,
        })
    }
    /// Select the common credentials file and CLI profile used by Doubao OAuth.
    /// Resolves relative `credentials_path` against the current directory and rejects invalid profiles.
    /// Returns configuration errors without reading credentials, keys or application secrets.
    pub fn with_credentials_store(
        mut self,
        credentials_path: PathBuf,
        profile: String,
    ) -> Result<Self> {
        crate::infra::credentials::validate_profile_name(&profile)
            .map_err(|_| Error::Configuration)?;
        self.credentials_path = if credentials_path.is_absolute() {
            credentials_path
        } else {
            std::env::current_dir()
                .map_err(|_| Error::Configuration)?
                .join(credentials_path)
        };
        if self.credentials_path.file_name().is_none() {
            return Err(Error::Configuration);
        }
        self.profile = profile;
        Ok(self)
    }
    /// Load the publisher application unless any application override variable is present.
    /// Callback and state-home overrides apply independently. Returns a redacted configuration
    /// error for incomplete or invalid applications; never reads a secret file or contacts OAuth.
    pub fn for_cli() -> Result<Self> {
        application::configure(
            application::ApplicationEnvironment::read(),
            application::compiled_application()?,
        )
    }
    /// Report whether this binary contains a complete, valid publisher application.
    /// Reads only compiled inputs; this does not assert provider permissions or platform support.
    pub fn has_bundled_application() -> bool {
        matches!(application::compiled_application(), Ok(Some(_)))
    }
    /// Load explicit app ID, optional callback and separate state home from the environment.
    /// The protected app-secret file is read only by `load_secret`, allowing status/logout.
    pub fn from_env(required_scopes: &[&str]) -> Result<Self> {
        let app_id =
            std::env::var("STORAGE_CLI_DOUBAO_APP_ID").map_err(|_| Error::Configuration)?;
        // [Review Fix #5] Only an absent callback selects the default; invalid Unicode is an error.
        let redirect = match std::env::var("STORAGE_CLI_DOUBAO_CALLBACK") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => "http://localhost:8080/callback".into(),
            Err(_) => return Err(Error::Configuration),
        };
        let home = std::env::var_os("STORAGE_CLI_ASK_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs_next::home_dir().map(|p| p.join(".tos/ask")))
            .ok_or(Error::Configuration)?;
        let mut config = Self::new(
            app_id,
            &redirect,
            required_scopes.iter().map(|s| (*s).into()).collect(),
            home,
        )?;
        config.secret_file =
            std::env::var_os("STORAGE_CLI_DOUBAO_APP_SECRET_FILE").map(PathBuf::from);
        Ok(config)
    }
    /// Copy the compiled secret into zeroizing owned storage, or read the protected override file.
    /// Rejects missing secrets, unsafe files and invalid contents without falling back to a bundle.
    pub fn load_secret(&self) -> Result<Secret> {
        if let Some(secret) = self.bundled_secret {
            return Ok(Secret::new(secret.into()));
        }
        let path = self.secret_file.as_ref().ok_or(Error::Configuration)?;
        let bytes = super::store::read_secret_file(path)?;
        // [Review Fix #6] Borrow from the zeroizing buffer so invalid UTF-8 owns no unprotected copy.
        let value = std::str::from_utf8(&bytes).map_err(|_| Error::Configuration)?;
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.len() > 4096 || trimmed.contains(['\r', '\n']) {
            return Err(Error::Configuration);
        }
        Ok(Secret::new(trimmed.into()))
    }
    /// Application identifier bound to all stored credentials.
    pub fn app_id(&self) -> &str {
        &self.app_id
    }
    /// Exact registered callback URI.
    pub fn redirect_uri(&self) -> &str {
        self.redirect_uri.as_str()
    }
}

fn validate_redirect(value: &str) -> Result<Url> {
    let uri = Url::parse(value).map_err(|_| Error::Configuration)?;
    let is_loopback = matches!(uri.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if uri.scheme() != "http"
        || !is_loopback
        || uri.port().is_none()
        || uri.port() == Some(0)
        || uri.path() != "/callback"
        || uri.query().is_some()
        || uri.fragment().is_some()
        || !uri.username().is_empty()
        || uri.password().is_some()
    {
        return Err(Error::Configuration);
    }
    Ok(uri)
}

pub(crate) fn validated_scopes(scope: &str) -> Result<Vec<&str>> {
    let mut scopes = Vec::new();
    for value in scope.split(' ') {
        if !valid_scope_token(value) || scopes.contains(&value) {
            return Err(Error::Response);
        }
        scopes.push(value);
    }
    Ok(scopes)
}
