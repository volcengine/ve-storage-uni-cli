use super::*;
use crate::{
    cleanup::{CleanupControl, TracingCleanupObserver},
    crypto::{Crypto, KeyMaterial},
    login::*,
};

struct UnavailableAdapters;
impl OAuthProvider for UnavailableAdapters {
    fn authorization_url(&self, _: AuthorizationRequest<'_>) -> Result<String, ProviderError> {
        Err(ProviderError::Unavailable)
    }
    fn exchange(&self, _: ExchangeRequest) -> ProviderFuture<'_> {
        Box::pin(ready(Err(ProviderError::Unavailable)))
    }
    fn refresh(&self, _: ProviderRefreshRequest) -> RefreshFuture<'_> {
        Box::pin(ready(Err(RefreshFailure::OutcomeUnknown)))
    }
}
impl AdmissionProvider for UnavailableAdapters {
    fn snapshot(&self) -> AdmissionFuture<'_> {
        Box::pin(ready(Err(AdmissionError)))
    }
}

fn ring(deployment: &str, app: &str) -> Arc<Crypto> {
    Arc::new(
        Crypto::new(
            deployment,
            app,
            vec![KeyMaterial::new(1, [1; 32], [2; 32]).unwrap()],
        )
        .unwrap(),
    )
}

fn service(store: Arc<Store>, crypto: Arc<Crypto>) -> LoginService {
    let configuration = EnabledConfig::new(
        DeploymentIdentity {
            app_id: crypto.app().into(),
            environment: "test".into(),
            deployment: crypto.deployment().into(),
        },
        1,
        vec!["read".into()],
        OAuthEndpoints {
            callback_url: "https://auth.example/v1/oauth/doubao/callback".into(),
            authorization_url: "https://oauth.example/authorize".into(),
        },
    )
    .unwrap();
    let unavailable = Arc::new(UnavailableAdapters);
    LoginService::new(
        store,
        crypto,
        configuration,
        LoginAdapters {
            provider: unavailable.clone(),
            admission: unavailable,
        },
    )
    .unwrap()
}

fn worker(
    store: Arc<CleanupStore>,
) -> (
    CleanupWorker,
    watch::Sender<Option<crate::cleanup::CleanupSettings>>,
) {
    let (control, publisher) = CleanupControl::channel();
    (
        CleanupWorker::new(store, control, Arc::new(TracingCleanupObserver)),
        publisher,
    )
}

#[tokio::test]
async fn constructor_rejects_different_app_deployment_or_copied_crypto_without_io() {
    let crypto = ring("deployment", "app");
    for other in [
        ring("deployment", "other-app"),
        ring("other-deployment", "app"),
        ring("deployment", "app"),
    ] {
        let (api, cleanup, pools) = crate::storage::lazy_runtime_stores(crypto.clone(), other);
        assert!(!pools.is_api_closed());
        assert!(!pools.is_cleanup_closed());
        let (worker, _publisher) = worker(cleanup.clone());
        let outcome = AuthorizationRuntime::new(service(api.clone(), crypto.clone()), worker);
        assert!(matches!(outcome, Err(RuntimeError::InvalidBinding)));
        assert!(pools.is_api_closed());
        assert!(pools.is_cleanup_closed());
        assert_eq!(
            api.session(uuid::Uuid::now_v7()).await.err(),
            Some(crate::storage::StorageError::Unavailable)
        );
        assert_eq!(
            cleanup.cutoff().await.err(),
            Some(crate::storage::StorageError::Unavailable)
        );
    }
}

#[tokio::test]
async fn actual_runtime_idle_shutdown_closes_its_exact_both_stores() {
    let crypto = ring("deployment", "app");
    let (api, cleanup, pools) = crate::storage::lazy_runtime_stores(crypto.clone(), crypto.clone());
    let (worker, _publisher) = worker(cleanup.clone());
    let runtime = AuthorizationRuntime::new(service(api.clone(), crypto), worker).unwrap();
    assert!(!pools.is_api_closed());
    assert!(!pools.is_cleanup_closed());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    assert_eq!(runtime.serve(listener, ready(Ok(()))).await, Ok(()));
    assert!(pools.is_api_closed());
    assert!(pools.is_cleanup_closed());
    assert_eq!(
        api.session(uuid::Uuid::now_v7()).await.err(),
        Some(crate::storage::StorageError::Unavailable)
    );
    assert_eq!(
        cleanup.cutoff().await,
        Err(crate::storage::StorageError::Unavailable)
    );
}

#[tokio::test]
async fn cancelling_actual_runtime_permanently_closes_its_exact_both_stores() {
    let crypto = ring("deployment", "app");
    let (api, cleanup, pools) = crate::storage::lazy_runtime_stores(crypto.clone(), crypto.clone());
    let (worker, _publisher) = worker(cleanup.clone());
    let runtime = AuthorizationRuntime::new(service(api.clone(), crypto), worker).unwrap();
    assert!(!pools.is_api_closed());
    assert!(!pools.is_cleanup_closed());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut serving = Box::pin(runtime.serve(listener, pending()));
    assert!(futures::poll!(&mut serving).is_pending());
    drop(serving);
    assert!(pools.is_api_closed());
    assert!(pools.is_cleanup_closed());
    assert_eq!(
        api.session(uuid::Uuid::now_v7()).await.err(),
        Some(crate::storage::StorageError::Unavailable)
    );
    assert_eq!(
        cleanup.cutoff().await,
        Err(crate::storage::StorageError::Unavailable)
    );
}

#[tokio::test]
async fn dropping_unpolled_serve_future_permanently_closes_both_actual_pools() {
    let crypto = ring("deployment", "app");
    let (api, cleanup, pools) = crate::storage::lazy_runtime_stores(crypto.clone(), crypto.clone());
    let (worker, _publisher) = worker(cleanup);
    let runtime = AuthorizationRuntime::new(service(api, crypto), worker).unwrap();
    assert!(!pools.is_api_closed());
    assert!(!pools.is_cleanup_closed());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let serving = runtime.serve(listener, pending());
    drop(serving);
    assert!(pools.is_api_closed());
    assert!(pools.is_cleanup_closed());
}

#[tokio::test]
async fn dropping_runtime_without_serving_permanently_closes_both_actual_pools() {
    let crypto = ring("deployment", "app");
    let (api, cleanup, pools) = crate::storage::lazy_runtime_stores(crypto.clone(), crypto.clone());
    let (worker, _publisher) = worker(cleanup);
    let runtime = AuthorizationRuntime::new(service(api, crypto), worker).unwrap();
    assert!(!pools.is_api_closed());
    assert!(!pools.is_cleanup_closed());
    drop(runtime);
    assert!(pools.is_api_closed());
    assert!(pools.is_cleanup_closed());
}
