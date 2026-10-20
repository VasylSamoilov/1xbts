//! SMSC node configuration (loaded from `config/smsc.json`).

use std::{net::SocketAddr, path::Path};

use serde::{Deserialize, Serialize};

/// SMSC node configuration (loaded from `config/smsc.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SmscNodeConfig {
    /// SMSC gRPC listen address.
    pub grpc_listen_addr: SocketAddr,
    #[serde(default)]
    pub postgres_dsn: Option<String>,
    #[serde(default = "default_hlr_endpoint")]
    pub hlr_endpoint: String,
    #[serde(default)]
    pub sweep: SweepSettings,
    #[serde(default)]
    pub smpp: Option<cdma_interconnect::InterconnectConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SweepSettings {
    #[serde(default = "default_sweep_enabled")]
    pub enabled: bool,
    #[serde(default = "default_retry_after_secs")]
    pub retry_after_secs: u64,
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    /// Timeout for an in-flight delivery whose MSC has not reported an outcome.
    #[serde(default = "default_stale_after_secs")]
    pub stale_after_secs: u64,
}

fn default_sweep_enabled() -> bool {
    true
}

fn default_retry_after_secs() -> u64 {
    10
}

fn default_interval_secs() -> u64 {
    10
}

fn default_stale_after_secs() -> u64 {
    120
}

impl Default for SweepSettings {
    fn default() -> Self {
        Self {
            enabled: default_sweep_enabled(),
            retry_after_secs: default_retry_after_secs(),
            interval_secs: default_interval_secs(),
            stale_after_secs: default_stale_after_secs(),
        }
    }
}

impl From<&SweepSettings> for crate::scheduler::SweepConfig {
    fn from(settings: &SweepSettings) -> Self {
        Self {
            enabled: settings.enabled,
            retry_after: std::time::Duration::from_secs(settings.retry_after_secs),
            interval: std::time::Duration::from_secs(settings.interval_secs),
            stale_after: std::time::Duration::from_secs(settings.stale_after_secs),
        }
    }
}

fn default_hlr_endpoint() -> String {
    "http://127.0.0.1:17019".to_string()
}

impl SmscNodeConfig {
    /// Load an `SmscNodeConfig` from a JSON file.
    pub fn load_from_path(path: &Path) -> Result<Self, std::io::Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        serde_json::from_value(merged)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SmscNodeConfig {
        SmscNodeConfig {
            grpc_listen_addr: "127.0.0.1:17020".parse().unwrap(),
            postgres_dsn: None,
            hlr_endpoint: default_hlr_endpoint(),
            sweep: SweepSettings::default(),
            smpp: None,
        }
    }

    #[test]
    fn default_has_no_dsn() {
        let cfg = test_config();
        assert!(cfg.postgres_dsn.is_none());
    }
}
