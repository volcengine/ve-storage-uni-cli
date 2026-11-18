use std::{
    ffi::OsString,
    fmt,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};
use tracing::level_filters::LevelFilter;

/// Bootstrap configuration; no credentials, database or provider settings exist.
#[derive(Clone, Debug)]
pub struct Config {
    /// Local listener address. Defaults to loopback port 8080.
    pub listen: SocketAddr,
    /// A single bounded log level, never an arbitrary filter expression.
    pub log_level: LevelFilter,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
            log_level: LevelFilter::INFO,
        }
    }
}

/// Sanitized configuration failure; setting values are never retained or printed.
#[derive(Debug)]
pub struct ConfigError;

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid authorization service configuration")
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Read `ASK_AUTH_LISTEN` and `ASK_AUTH_LOG_LEVEL` from the environment.
    ///
    /// Returns validated configuration. Unknown `ASK_AUTH_` settings, non-Unicode
    /// values, invalid socket addresses and log filter expressions return
    /// `ConfigError`, without retaining the rejected values. No I/O is performed.
    pub fn from_environment() -> Result<Self, ConfigError> {
        Self::from_settings(std::env::vars_os())
    }

    fn from_settings(
        settings: impl Iterator<Item = (OsString, OsString)>,
    ) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        for (name, value) in settings {
            let Some(name) = name.to_str().filter(|name| name.starts_with("ASK_AUTH_")) else {
                continue;
            };
            let value = value.to_str().ok_or(ConfigError)?;
            match name {
                "ASK_AUTH_LISTEN" => config.listen = value.parse().map_err(|_| ConfigError)?,
                "ASK_AUTH_LOG_LEVEL" => config.log_level = parse_log_level(value)?,
                _ => return Err(ConfigError),
            }
        }
        Ok(config)
    }
}

fn parse_log_level(value: &str) -> Result<LevelFilter, ConfigError> {
    match value {
        "off" => Ok(LevelFilter::OFF),
        "error" => Ok(LevelFilter::ERROR),
        "warn" => Ok(LevelFilter::WARN),
        "info" => Ok(LevelFilter::INFO),
        "debug" => Ok(LevelFilter::DEBUG),
        "trace" => Ok(LevelFilter::TRACE),
        _ => Err(ConfigError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_configuration_is_loopback_and_info() {
        let config = Config::from_settings(std::iter::empty()).unwrap();
        assert_eq!(
            config.listen,
            "127.0.0.1:8080".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(config.log_level, LevelFilter::INFO);
    }

    #[test]
    fn explicit_ipv6_address_and_each_bounded_log_level_are_supported() {
        for level in ["off", "error", "warn", "info", "debug", "trace"] {
            let settings = [
                ("ASK_AUTH_LISTEN", "[::1]:0"),
                ("ASK_AUTH_LOG_LEVEL", level),
            ];
            let config = Config::from_settings(
                settings
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into())),
            )
            .unwrap();
            assert_eq!(config.listen, "[::1]:0".parse::<SocketAddr>().unwrap());
        }
    }
}
