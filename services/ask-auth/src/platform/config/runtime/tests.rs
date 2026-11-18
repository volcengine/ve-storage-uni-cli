use super::*;
fn platform() -> BTreeMap<String, String> {
    [("PSM", "security.ask_auth"), ("RUNTIME_IDC_NAME", "boe")]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
}
fn context(values: &BTreeMap<String, String>) -> Result<RuntimeContext, TccError> {
    RuntimeContext::from_map(values, ExpectedEnvironment::Boe)
}
#[test]
fn runtime_identity_precedence_and_discovery_are_explicit() {
    let mut values = platform();
    values.extend([
        ("LOAD_SERVICE_PSM".into(), "load.service.psm".into()),
        ("TCE_PSM".into(), "fallback.service.psm".into()),
        ("SERVICE_ENV".into(), "staging".into()),
        ("SERVICE_CLUSTER".into(), "sender-cluster".into()),
        ("TCE_CLUSTER".into(), "unused-cluster".into()),
    ]);
    let runtime = context(&values).ok().unwrap();
    assert_eq!(runtime.headers["x-psm"], "load.service.psm");
    assert_eq!(runtime.headers["x-env"], "staging");
    assert_eq!(runtime.headers["x-cluster"], "sender-cluster");
    let request = runtime.discovery_request();
    assert_eq!(request.psm, "bytedance.bconfig.envs_proxy");
    assert_eq!(request.cluster.as_deref(), Some("default"));
    assert_eq!(request.idc.as_deref(), Some("boe"));
}
#[test]
fn proxy_selection_preserves_bconfig_v1_rules() {
    for (environment, proxy) in [
        ("", "proxy"),
        ("prod", "proxy"),
        ("canary", "proxy"),
        ("boe", "envs_proxy"),
        ("staging", "envs_proxy"),
    ] {
        let mut values = platform();
        values.insert("SERVICE_ENV".into(), environment.into());
        assert_eq!(
            context(&values).ok().unwrap().discovery_request().psm,
            format!("bytedance.bconfig.{proxy}")
        );
    }
    let runtime = context(&platform()).ok().unwrap();
    assert_eq!(runtime.headers["x-env"], "prod");
    assert_eq!(runtime.headers["x-cluster"], "default");
}
#[test]
fn runtime_classification_rejects_unknown_mismatch_ppe_and_offline() {
    for idc in ["boe", "boe2", "boei18n"] {
        let mut values = platform();
        values.insert("RUNTIME_IDC_NAME".into(), idc.into());
        assert!(context(&values).is_ok());
        assert!(RuntimeContext::from_map(&values, ExpectedEnvironment::Production).is_err());
    }
    for (key, value) in [
        ("RUNTIME_IDC_NAME", ""),
        ("RUNTIME_IDC_NAME", "unknown"),
        ("RUNTIME_IDC_NAME", "../boe"),
        ("RUNTIME_IDC_NAME", "lq"),
        ("TCE_HOST_ENV", "ppe"),
        ("TESTING_PREFIX", "offline"),
        ("PSM", "-"),
    ] {
        let mut values = platform();
        values.insert(key.into(), value.into());
        assert!(context(&values).is_err());
    }
    let mut values = platform();
    values.remove("RUNTIME_IDC_NAME");
    assert!(context(&values).is_err());
}
#[test]
fn production_requires_explicit_platform_evidence() {
    let mut values = platform();
    values.insert("RUNTIME_IDC_NAME".into(), "lq".into());
    assert!(RuntimeContext::from_map(&values, ExpectedEnvironment::Production).is_err());
    values.insert("IS_PROD_RUNTIME".into(), "".into());
    assert!(RuntimeContext::from_map(&values, ExpectedEnvironment::Production).is_ok());
    values.remove("IS_PROD_RUNTIME");
    values.insert("TCE_ENV".into(), "prod".into());
    assert!(RuntimeContext::from_map(&values, ExpectedEnvironment::Production).is_err());
    values.insert("SERVICE_ENV".into(), "prod".into());
    assert!(RuntimeContext::from_map(&values, ExpectedEnvironment::Production).is_ok());
}
#[test]
fn runtime_metadata_is_bounded_and_header_safe() {
    for (key, value) in [
        ("PSM", "a..b"),
        ("LOAD_SERVICE_PSM", ""),
        ("TCE_STAGE", "bad\r\nx-evil: header"),
        ("MY_POD_NAME", "bad\0pod"),
        ("SERVICE_CLUSTER", ""),
        ("MY_HOST_IP", "hostname.example"),
        ("MY_HOST_IPV6", "not-ipv6"),
    ] {
        let mut values = platform();
        values.insert(key.into(), value.into());
        assert!(context(&values).is_err());
    }
    let mut values = platform();
    values.insert("MY_POD_NAME".into(), "p".repeat(257));
    assert!(context(&values).is_err());
    values.insert("MY_POD_NAME".into(), "pod-123".into());
    values.insert("MY_HOST_IPV6".into(), "fd00::1".into());
    values.insert("TCE_STAGE".into(), "canary".into());
    let runtime = context(&values).ok().unwrap();
    assert_eq!(runtime.headers["x-host-ip"], "fd00::1");
    assert_eq!(runtime.headers["x-pod"], "pod-123");
    assert!(runtime.headers["x-sdk-version"]
        .to_str()
        .unwrap()
        .starts_with("ask-auth/"));
}
