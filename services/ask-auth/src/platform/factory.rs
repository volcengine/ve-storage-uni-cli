use crate::platform::{
    control::{ConfigSource, Controls, Fingerprint, HostSnapshot, LiveAdmission, Publisher},
    settings::{self, ActiveSettings, Secrets},
    AppError,
};
use crate::{
    cleanup::{CleanupControl, CleanupWorker, TracingCleanupObserver},
    crypto::Crypto,
    doubao::{AppCredentials, DoubaoOAuth},
    login::{DeploymentIdentity, EnabledConfig, LoginAdapters, LoginService},
    runtime::AuthorizationRuntime,
    storage::{CleanupDatabase, CleanupStore, DatabaseConfig, RuntimeDatabase, Store, Tls},
};
use std::{sync::Arc, time::Duration};

struct Prepared {
    active: ActiveSettings,
    crypto: Arc<Crypto>,
    provider: Arc<DoubaoOAuth>,
    config: EnabledConfig,
    api: Option<DatabaseConfig>,
    cleanup: DatabaseConfig,
}
impl Prepared {
    fn new(active: ActiveSettings, snapshot: &HostSnapshot) -> Result<Self, AppError> {
        let mut secrets = Secrets::parse(
            settings::required(snapshot.secrets.as_deref())?,
            active.current_key_version,
        )?;
        let crypto = Arc::new(
            Crypto::new(&active.deployment, &active.app_id, secrets.keys)
                .map_err(|_| AppError::Configuration)?,
        );
        let provider = provider(&active, secrets.client_secret)?;
        let config = enabled_config(&active, &provider)?;
        let api = database_config(
            &active,
            secrets.database_url.to_string(),
            active.api_pool_connections,
        )?;
        let cleanup = database_config(
            &active,
            std::mem::take(&mut secrets.database_url),
            active.cleanup_pool_connections,
        )?;
        Ok(Self {
            active,
            crypto,
            provider,
            config,
            api: Some(api),
            cleanup,
        })
    }
}
// [Review Fix #84] Named guard state makes ownership transfer explicit; failure/cancellation keeps closure armed.
struct ApiGuard {
    store: Arc<Store>,
    is_armed: bool,
}
impl Drop for ApiGuard {
    fn drop(&mut self) {
        if self.is_armed {
            drop(self.store.close());
        }
    }
}
struct CleanupGuard {
    store: Arc<CleanupStore>,
    is_armed: bool,
}
impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.is_armed {
            drop(self.store.close());
        }
    }
}
struct Pools {
    api: ApiGuard,
    cleanup: Option<CleanupGuard>,
}
impl Pools {
    async fn close(&self) {
        let close = async {
            if let Some(cleanup) = &self.cleanup {
                tokio::join!(self.api.store.close(), cleanup.store.close());
            } else {
                self.api.store.close().await;
            }
        };
        // The guards synchronously mark closed if acknowledgements exceed this deadline.
        let _ = tokio::time::timeout(Duration::from_secs(3), close).await;
    }
}
pub(crate) struct EnabledHost {
    pub(crate) runtime: AuthorizationRuntime,
    pub(crate) publisher: Publisher,
}
pub(crate) fn validate_active(
    active: ActiveSettings,
    snapshot: &HostSnapshot,
) -> Result<(), AppError> {
    Prepared::new(active, snapshot).map(|_| ())
}
pub(crate) async fn build(
    active: ActiveSettings,
    initial: &HostSnapshot,
    source: Arc<dyn ConfigSource>,
    environment: crate::platform::config::ExpectedEnvironment,
) -> Result<EnabledHost, AppError> {
    crate::platform::bootstrap::validate_controls(initial)?;
    // [Review Fix #78] Validate provider, cryptography and both TLS configurations before connecting either pool.
    let mut prepared = Prepared::new(active, initial)?;
    let database = RuntimeDatabase::connect(prepared.api.take().ok_or(AppError::Configuration)?)
        .await
        .map_err(|_| AppError::Database)?;
    let api = Arc::new(
        Store::new(database, &prepared.active.app_id, prepared.crypto.clone())
            .map_err(|_| AppError::Configuration)?,
    );
    let mut pools = Pools {
        api: ApiGuard {
            store: api,
            is_armed: true,
        },
        cleanup: None,
    };
    let result = complete(prepared, initial, source, environment, &mut pools).await;
    if result.is_err() {
        pools.close().await;
    }
    if result.is_ok() {
        pools.api.is_armed = false;
        if let Some(cleanup) = &mut pools.cleanup {
            cleanup.is_armed = false;
        }
    }
    result
}
async fn complete(
    prepared: Prepared,
    initial: &HostSnapshot,
    source: Arc<dyn ConfigSource>,
    environment: crate::platform::config::ExpectedEnvironment,
    pools: &mut Pools,
) -> Result<EnabledHost, AppError> {
    let database = CleanupDatabase::connect(prepared.cleanup)
        .await
        .map_err(|_| AppError::Database)?;
    let cleanup = Arc::new(
        CleanupStore::new(database, &prepared.active.app_id, prepared.crypto.clone())
            .map_err(|_| AppError::Configuration)?,
    );
    pools.cleanup = Some(CleanupGuard {
        store: cleanup.clone(),
        is_armed: true,
    });
    let (publisher, admission, cleanup_control) =
        publisher(source, cleanup.clone(), initial, environment).await?;
    let adapters = LoginAdapters {
        provider: prepared.provider,
        admission,
    };
    let service = LoginService::new(
        pools.api.store.clone(),
        prepared.crypto,
        prepared.config,
        adapters,
    )
    .map_err(|_| AppError::Configuration)?;
    let worker = CleanupWorker::new(cleanup, cleanup_control, Arc::new(TracingCleanupObserver));
    let runtime = AuthorizationRuntime::new(service, worker).map_err(|_| AppError::Runtime)?;
    Ok(EnabledHost { runtime, publisher })
}
fn provider(
    active: &ActiveSettings,
    secret: crate::doubao::SecretString,
) -> Result<Arc<DoubaoOAuth>, AppError> {
    let credentials =
        AppCredentials::new(active.app_id.clone(), secret).map_err(|_| AppError::Configuration)?;
    DoubaoOAuth::new(
        credentials,
        &active.callback_url,
        active.required_scopes.clone(),
    )
    .map(Arc::new)
    .map_err(|_| AppError::Configuration)
}
fn enabled_config(
    active: &ActiveSettings,
    provider: &DoubaoOAuth,
) -> Result<EnabledConfig, AppError> {
    let identity = DeploymentIdentity {
        app_id: active.app_id.clone(),
        environment: active.environment.clone(),
        deployment: active.deployment.clone(),
    };
    EnabledConfig::new(
        identity,
        active.current_key_version,
        active.required_scopes.clone(),
        provider.endpoints(),
    )
    .map_err(|_| AppError::Configuration)
}
fn database_config(
    active: &ActiveSettings,
    url: String,
    maximum: u32,
) -> Result<DatabaseConfig, AppError> {
    DatabaseConfig::new(
        url,
        Tls::VerifyIdentity {
            ca: active.database_ca_path.clone(),
        },
        maximum,
    )
    .map_err(|_| AppError::Configuration)
}
async fn publisher(
    source: Arc<dyn ConfigSource>,
    cleanup: Arc<CleanupStore>,
    initial: &HostSnapshot,
    environment: crate::platform::config::ExpectedEnvironment,
) -> Result<(Publisher, Arc<LiveAdmission>, CleanupControl), AppError> {
    let (admission, admission_sender) = LiveAdmission::channel();
    let (cleanup_control, cleanup_sender) = CleanupControl::channel();
    let publisher = Publisher {
        source,
        clock: cleanup.clone(),
        controls: Controls {
            admission: admission_sender,
            cleanup: cleanup_sender,
        },
        fingerprint: Fingerprint::capture(initial),
        environment,
    };
    let fresh = publisher.source.read().await?;
    if !publisher.fingerprint.matches(&fresh) {
        return Err(AppError::RestartRequired);
    }
    publisher.seed(&fresh).await?;
    Ok((publisher, admission, cleanup_control))
}
// [Review Fix #104] Protected file fixtures require Unix open options; unsupported hosts fail closed.
#[cfg(all(test, unix))]
mod tests;
