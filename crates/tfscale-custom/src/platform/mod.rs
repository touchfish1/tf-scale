use crate::tun::{PlatformTunDevice, TunConfig};
use tfscale_net::Result;

#[cfg(any(test, target_os = "linux"))]
pub(crate) mod linux;

#[cfg(any(test, target_os = "macos"))]
pub(crate) mod macos;

#[cfg(target_os = "linux")]
pub(crate) fn configure_tun(config: &TunConfig) -> Result<PlatformTunDevice> {
    linux::configure_tun(config)
}

#[cfg(target_os = "macos")]
pub(crate) fn configure_tun(config: &TunConfig) -> Result<PlatformTunDevice> {
    macos::configure_tun(config)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn configure_tun(config: &TunConfig) -> Result<PlatformTunDevice> {
    let _ = config;
    Err(crate::tun::unsupported_platform_error())
}
