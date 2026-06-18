use std::net::Ipv4Addr;
use tfscale_net::{BackendError, LocalBackendConfig, Result};

const OVERLAY_CIDR: &str = "100.64.0.0/10";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TunConfig {
    pub interface_name: String,
    pub overlay_ip: Ipv4Addr,
    pub listen_port: u16,
    pub overlay_cidr: String,
}

impl TunConfig {
    pub fn from_local_config(config: &LocalBackendConfig) -> Self {
        Self {
            interface_name: config.interface_name.clone(),
            overlay_ip: config.overlay_ip,
            listen_port: config.listen_port,
            overlay_cidr: OVERLAY_CIDR.to_string(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TunStatus {
    pub configured: bool,
    pub interface_name: String,
    pub message: Option<String>,
}

impl TunStatus {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn configured(interface_name: impl Into<String>) -> Self {
        Self {
            configured: true,
            interface_name: interface_name.into(),
            message: None,
        }
    }

    pub fn failed(interface_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            configured: false,
            interface_name: interface_name.into(),
            message: Some(message.into()),
        }
    }
}

pub(crate) fn configure_tun(config: &TunConfig) -> Result<TunStatus> {
    crate::platform::configure_tun(config)
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn unsupported_platform_error() -> BackendError {
    BackendError::UnsupportedPlatform(format!(
        "TUN setup is not supported on {}",
        std::env::consts::OS
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_tun_config_from_local_backend_config() {
        let config = TunConfig::from_local_config(&LocalBackendConfig {
            interface_name: "tfscale0".to_string(),
            overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
            listen_port: 51820,
        });

        assert_eq!(config.interface_name, "tfscale0");
        assert_eq!(config.overlay_ip, Ipv4Addr::new(100, 64, 0, 2));
        assert_eq!(config.listen_port, 51820);
        assert_eq!(config.overlay_cidr, "100.64.0.0/10");
    }
}
