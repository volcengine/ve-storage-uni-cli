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

//! Integration tests for the current unified ConfigFile/Profile model.
//! [Review Fix #3] Align config tests with the current unified ConfigFile/Profile model.

use tos_core::infra::config::{
    derive_tos_control_endpoint, Binary, ConfigFile, FieldSource, Profile, TosOverride,
    DEFAULT_TOS_BATCH_REPORT_DIR, DEFAULT_TOS_BATCH_REPORT_FORMAT, DEFAULT_TOS_CHECKPOINT_DIR,
    DEFAULT_TOS_PROGRESS_ENABLED,
};

static BYTE_TOS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_config_path_ends_with_config_toml() {
    let path = ConfigFile::config_path();
    assert!(path.ends_with(".tos/config.toml"));
}

#[test]
fn test_config_file_default_has_no_profiles() {
    let config = ConfigFile::default();
    assert!(config.profiles.is_empty());
}

#[test]
fn test_config_file_deserializes_profile_and_tos_override() {
    let toml_str = r#"
[default]
region = "cn-beijing"
access_key_id = "AK123"
secret_access_key = "SK456"

[default.tos]
endpoint = "https://tos-cn-beijing.volces.com"
control_endpoint = "https://tos-control-cn-beijing.volces.com"
"#;
    let config: ConfigFile = toml::from_str(toml_str).unwrap();
    let profile = config.get_profile("default").unwrap();
    assert_eq!(profile.region.as_deref(), Some("cn-beijing"));
    assert_eq!(profile.access_key_id.as_deref(), Some("AK123"));
    let tos = profile.tos.as_ref().unwrap();
    assert_eq!(
        tos.endpoint.as_deref(),
        Some("https://tos-cn-beijing.volces.com")
    );
}

#[test]
fn test_set_by_path_writes_shared_and_binary_fields() {
    let mut config = ConfigFile::default();
    config
        .set_by_path(&["default", "region"], "cn-shanghai")
        .unwrap();
    config
        .set_by_path(
            &["default", "tos", "endpoint"],
            "https://tos-cn-shanghai.volces.com",
        )
        .unwrap();

    let profile = config.get_profile("default").unwrap();
    assert_eq!(profile.region.as_deref(), Some("cn-shanghai"));
    assert_eq!(
        profile.tos.as_ref().unwrap().endpoint.as_deref(),
        Some("https://tos-cn-shanghai.volces.com")
    );
}

#[test]
fn test_effective_profile_prefers_binary_override() {
    let mut config = ConfigFile::default();
    config.profiles.insert(
        "default".into(),
        Profile {
            region: Some("cn-beijing".into()),
            endpoint: Some("https://shared.example.com".into()),
            access_key_id: Some("AK".into()),
            secret_access_key: Some("SK".into()),
            ve_tos: Some(TosOverride {
                endpoint: Some("https://tos-cn-beijing.volces.com".into()),
                control_endpoint: Some("https://tos-control-cn-beijing.volces.com".into()),
                ..TosOverride::default()
            }),
            ..Profile::default()
        },
    );

    let effective = config
        .get_effective_profile("default", Binary::VeTos)
        .expect("effective profile");
    assert_eq!(
        effective.endpoint.value.as_deref(),
        Some("https://tos-cn-beijing.volces.com")
    );
    assert_eq!(effective.endpoint.source, FieldSource::BinaryOverride);
    assert_eq!(
        effective.control_endpoint.value.as_deref(),
        Some("https://tos-control-cn-beijing.volces.com")
    );
}

#[test]
fn test_tos_effective_profile_hides_control_endpoint() {
    let mut config = ConfigFile::default();
    config.profiles.insert(
        "default".into(),
        Profile {
            region: Some("cn-beijing".into()),
            endpoint: Some("https://tos-cn-beijing.volces.com".into()),
            access_key_id: Some("AK".into()),
            secret_access_key: Some("SK".into()),
            tos: Some(TosOverride {
                endpoint: Some("https://tos-cn-beijing.volces.com".into()),
                control_endpoint: Some("https://tos-control-cn-beijing.volces.com".into()),
                ..TosOverride::default()
            }),
            ..Profile::default()
        },
    );

    let effective = config
        .get_effective_profile("default", Binary::Tos)
        .expect("effective profile");
    assert_eq!(effective.control_endpoint.value, None);
    assert_eq!(effective.control_endpoint.source, FieldSource::Unset);
}

#[test]
fn test_tos_effective_profile_includes_psm_bns_fields() {
    let mut config = ConfigFile::default();
    config.profiles.insert(
        "default".into(),
        Profile {
            region: Some("cn-beijing".into()),
            access_key_id: Some("AK".into()),
            secret_access_key: Some("SK".into()),
            tos: Some(TosOverride {
                psm: Some("toutiao.tos.tosapi".into()),
                idc: Some("lf".into()),
                cluster: Some("default".into()),
                addr_family: Some("v4".into()),
                ..TosOverride::default()
            }),
            ..Profile::default()
        },
    );

    let effective = config
        .get_effective_profile("default", Binary::Tos)
        .expect("effective profile");

    assert_eq!(effective.psm.value.as_deref(), Some("toutiao.tos.tosapi"));
    assert_eq!(effective.psm.source, FieldSource::BinaryOverride);
    assert_eq!(effective.idc.value.as_deref(), Some("lf"));
    assert_eq!(effective.cluster.value.as_deref(), Some("default"));
    assert_eq!(effective.addr_family.value.as_deref(), Some("v4"));
}

#[test]
fn test_tos_psm_modifiers_are_ineffective_without_psm() {
    let mut config = ConfigFile::default();
    config.profiles.insert(
        "default".into(),
        Profile {
            region: Some("cn-beijing".into()),
            access_key_id: Some("AK".into()),
            secret_access_key: Some("SK".into()),
            tos: Some(TosOverride {
                idc: Some("lf".into()),
                cluster: Some("default".into()),
                addr_family: Some("v4".into()),
                ..TosOverride::default()
            }),
            ..Profile::default()
        },
    );

    let effective = config
        .get_effective_profile("default", Binary::Tos)
        .expect("effective profile");

    assert_eq!(effective.psm.value, None);
    assert_eq!(effective.idc.value, None);
    assert_eq!(effective.cluster.value, None);
    assert_eq!(effective.addr_family.value, None);
}

#[test]
fn test_byte_tos_env_psm_modifiers_require_non_empty_psm() {
    let _guard = BYTE_TOS_ENV_LOCK.lock().expect("BYTE_TOS env lock");
    let old_values: Vec<(&str, Option<String>)> = [
        "BYTE_TOS_PSM",
        "BYTE_TOS_IDC",
        "BYTE_TOS_CLUSTER",
        "BYTE_TOS_ADDR_FAMILY",
    ]
    .into_iter()
    .map(|name| (name, std::env::var(name).ok()))
    .collect();

    std::env::set_var("BYTE_TOS_PSM", " ");
    std::env::set_var("BYTE_TOS_IDC", "lf");
    std::env::set_var("BYTE_TOS_CLUSTER", "default");
    std::env::set_var("BYTE_TOS_ADDR_FAMILY", "v4");

    let profile = Profile::from_byte_tos_env();
    assert_eq!(profile.psm, None);
    assert_eq!(profile.idc, None);
    assert_eq!(profile.cluster, None);
    assert_eq!(profile.addr_family, None);

    for (name, value) in old_values {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }
}

#[test]
fn test_ve_tos_effective_profile_ignores_tos_psm_fields() {
    let mut config = ConfigFile::default();
    config.profiles.insert(
        "default".into(),
        Profile {
            region: Some("cn-beijing".into()),
            access_key_id: Some("AK".into()),
            secret_access_key: Some("SK".into()),
            tos: Some(TosOverride {
                psm: Some("toutiao.tos.tosapi".into()),
                idc: Some("lf".into()),
                cluster: Some("default".into()),
                addr_family: Some("v4".into()),
                ..TosOverride::default()
            }),
            ve_tos: Some(TosOverride {
                endpoint: Some("tos-cn-beijing.volces.com".into()),
                ..TosOverride::default()
            }),
            ..Profile::default()
        },
    );

    let effective = config
        .get_effective_profile("default", Binary::VeTos)
        .expect("effective profile");

    assert_eq!(effective.psm.value, None);
    assert_eq!(effective.idc.value, None);
    assert_eq!(effective.cluster.value, None);
    assert_eq!(effective.addr_family.value, None);
    assert_eq!(
        effective.endpoint.value.as_deref(),
        Some("tos-cn-beijing.volces.com")
    );
}

#[test]
fn test_tos_high_level_path_defaults_are_available() {
    let mut config = ConfigFile::default();
    config.profiles.insert(
        "default".into(),
        Profile {
            region: Some("cn-beijing".into()),
            endpoint: Some("https://tos-cn-beijing.volces.com".into()),
            access_key_id: Some("AK".into()),
            secret_access_key: Some("SK".into()),
            ..Profile::default()
        },
    );

    let effective = config
        .get_effective_profile("default", Binary::Tos)
        .expect("effective profile");

    assert_eq!(
        effective.checkpoint_dir.value.as_deref(),
        Some(DEFAULT_TOS_CHECKPOINT_DIR)
    );
    assert_eq!(effective.checkpoint_dir.source, FieldSource::Derived);
    assert_eq!(
        effective.batch_report_dir.value.as_deref(),
        Some(DEFAULT_TOS_BATCH_REPORT_DIR)
    );
    assert_eq!(
        effective.batch_report_format.value.as_deref(),
        Some(DEFAULT_TOS_BATCH_REPORT_FORMAT)
    );
    assert_eq!(
        effective.progress_enabled.value,
        Some(DEFAULT_TOS_PROGRESS_ENABLED)
    );
    assert_eq!(effective.progress_enabled.source, FieldSource::Derived);
}

#[test]
fn test_tos_high_level_path_defaults_can_be_overridden() {
    let mut config = ConfigFile::default();
    config
        .set_by_path(
            &["default", "tos", "checkpoint_dir"],
            "/var/tos/checkpoints",
        )
        .unwrap();
    config
        .set_by_path(&["default", "tos", "batch_report_dir"], "/var/tos/reports")
        .unwrap();
    config
        .set_by_path(&["default", "tos", "batch_report_format"], "csv")
        .unwrap();

    let effective = config
        .get_effective_profile("default", Binary::Tos)
        .expect("effective profile");

    assert_eq!(
        effective.checkpoint_dir.value.as_deref(),
        Some("/var/tos/checkpoints")
    );
    assert_eq!(effective.checkpoint_dir.source, FieldSource::BinaryOverride);
    assert_eq!(
        effective.batch_report_dir.value.as_deref(),
        Some("/var/tos/reports")
    );
    assert_eq!(effective.batch_report_format.value.as_deref(), Some("csv"));
}

#[test]
fn test_tos_progress_enabled_can_be_overridden() {
    let mut config = ConfigFile::default();
    config
        .set_by_path(&["default", "tos", "progress_enabled"], "false")
        .unwrap();

    let effective = config
        .get_effective_profile("default", Binary::Tos)
        .expect("effective profile");

    assert_eq!(effective.progress_enabled.value, Some(false));
    assert_eq!(
        effective.progress_enabled.source,
        FieldSource::BinaryOverride
    );
}

#[test]
fn test_derive_tos_control_endpoint_from_data_endpoint() {
    assert_eq!(
        derive_tos_control_endpoint(Some("https://tos-cn-beijing.volces.com")),
        Some("https://tos-control-cn-beijing.volces.com".to_string())
    );
    assert_eq!(
        derive_tos_control_endpoint(Some("https://example.com")),
        None
    );
}
