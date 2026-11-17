use super::{required, schema, text};
use crate::platform::{json, AppError};
use crate::{cleanup::CleanupSettings, login::AdmissionSnapshot};
use chrono::NaiveDateTime;
use serde::Deserialize;
use std::{collections::BTreeSet, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdmissionSettings {
    schema_version: u16,
    is_enabled: bool,
    allowed_users: Vec<String>,
    allowed_tenants: Vec<String>,
    denied_users: Vec<String>,
    denied_tenants: Vec<String>,
}
impl AdmissionSettings {
    pub(crate) fn parse(value: &str) -> Result<Self, AppError> {
        let settings: Self = json::parse(value)?;
        schema(settings.schema_version)?;
        let groups = [
            &settings.allowed_users,
            &settings.allowed_tenants,
            &settings.denied_users,
            &settings.denied_tenants,
        ];
        if groups.iter().map(|group| group.len()).sum::<usize>() > 10000 {
            return Err(AppError::Configuration);
        }
        for group in groups {
            let unique: BTreeSet<_> = group.iter().collect();
            if unique.len() != group.len() || !group.iter().all(|value| text(value, 256)) {
                return Err(AppError::Configuration);
            }
        }
        Ok(settings)
    }
    pub(crate) fn snapshot(self, version: String, fetched_at: NaiveDateTime) -> AdmissionSnapshot {
        AdmissionSnapshot {
            is_enabled: self.is_enabled,
            version,
            fetched_at,
            allowed_users: self.allowed_users.into_iter().collect(),
            allowed_tenants: self.allowed_tenants.into_iter().collect(),
            denied_users: self.denied_users.into_iter().collect(),
            denied_tenants: self.denied_tenants.into_iter().collect(),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CleanupPolicy {
    schema_version: u16,
    is_enabled: bool,
    interval_seconds: Option<u64>,
    batch: Option<u16>,
    pass_budget_ms: Option<u64>,
    transaction_budget_ms: Option<u64>,
    rows_per_second: Option<u16>,
}
impl CleanupPolicy {
    pub(crate) fn parse(value: &str) -> Result<Option<CleanupSettings>, AppError> {
        let fields: Self = json::parse(value)?;
        schema(fields.schema_version)?;
        if !fields.is_enabled {
            return Ok(None);
        }
        CleanupSettings::new(
            Duration::from_secs(required(fields.interval_seconds)?),
            required(fields.batch)?,
            Duration::from_millis(required(fields.pass_budget_ms)?),
            Duration::from_millis(required(fields.transaction_budget_ms)?),
            required(fields.rows_per_second)?,
        )
        .map(Some)
        .map_err(|_| AppError::Configuration)
    }
}
