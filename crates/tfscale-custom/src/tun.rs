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
    pub io_ready: bool,
    pub message: Option<String>,
}

impl TunStatus {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn configured(interface_name: impl Into<String>) -> Self {
        Self {
            configured: true,
            interface_name: interface_name.into(),
            io_ready: true,
            message: None,
        }
    }

    pub fn failed(interface_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            configured: false,
            interface_name: interface_name.into(),
            io_ready: false,
            message: Some(message.into()),
        }
    }
}

pub(crate) struct TunDevice {
    inner: PlatformTunDevice,
}

impl TunDevice {
    pub fn configure(config: &TunConfig) -> Result<Self> {
        Ok(Self {
            inner: crate::platform::configure_tun(config)?,
        })
    }

    pub fn status(&self) -> TunStatus {
        self.inner.status()
    }

    #[allow(dead_code)]
    pub fn read_packet(&self, buffer: &mut [u8]) -> Result<usize> {
        self.inner.read_packet(buffer)
    }

    pub fn try_read_packet(&self, buffer: &mut [u8]) -> Result<Option<usize>> {
        self.inner.try_read_packet(buffer)
    }

    #[allow(dead_code)]
    pub fn write_packet(&self, packet: &[u8]) -> Result<usize> {
        self.inner.write_packet(packet)
    }

    pub fn shutdown(self) -> Result<()> {
        self.inner.shutdown()
    }
}

pub(crate) struct PlatformTunDevice {
    #[cfg(target_os = "linux")]
    pub(crate) inner: crate::platform::linux::LinuxTunDevice,
    #[cfg(target_os = "macos")]
    pub(crate) inner: crate::platform::macos::MacosTunDevice,
}

impl PlatformTunDevice {
    pub fn status(&self) -> TunStatus {
        #[cfg(target_os = "linux")]
        {
            self.inner.status()
        }

        #[cfg(target_os = "macos")]
        {
            self.inner.status()
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            TunStatus::failed("unsupported", unsupported_platform_error().to_string())
        }
    }

    pub fn read_packet(&self, buffer: &mut [u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            self.inner.read_packet(buffer)
        }

        #[cfg(target_os = "macos")]
        {
            self.inner.read_packet(buffer)
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = buffer;
            Err(unsupported_platform_error())
        }
    }

    pub fn try_read_packet(&self, buffer: &mut [u8]) -> Result<Option<usize>> {
        #[cfg(target_os = "linux")]
        {
            self.inner.try_read_packet(buffer)
        }

        #[cfg(target_os = "macos")]
        {
            self.inner.try_read_packet(buffer)
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = buffer;
            Err(unsupported_platform_error())
        }
    }

    pub fn write_packet(&self, packet: &[u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            self.inner.write_packet(packet)
        }

        #[cfg(target_os = "macos")]
        {
            self.inner.write_packet(packet)
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = packet;
            Err(unsupported_platform_error())
        }
    }

    pub fn shutdown(self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.inner.shutdown()
        }

        #[cfg(target_os = "macos")]
        {
            self.inner.shutdown()
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(unsupported_platform_error())
        }
    }
}

#[cfg_attr(any(target_os = "linux", target_os = "macos"), allow(dead_code))]
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
            device_id: "dev_test".to_string(),
            interface_name: "tfscale0".to_string(),
            overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
            listen_port: 51820,
        });

        assert_eq!(config.interface_name, "tfscale0");
        assert_eq!(config.overlay_ip, Ipv4Addr::new(100, 64, 0, 2));
        assert_eq!(config.listen_port, 51820);
        assert_eq!(config.overlay_cidr, "100.64.0.0/10");
    }

    #[test]
    fn failed_status_is_not_io_ready() {
        let status = TunStatus::failed("tfscale0", "not ready");

        assert!(!status.configured);
        assert!(!status.io_ready);
        assert_eq!(status.interface_name, "tfscale0");
    }
}
