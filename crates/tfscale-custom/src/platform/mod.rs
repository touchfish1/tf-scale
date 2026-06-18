use crate::tun::{TunConfig, TunStatus};
use tfscale_net::Result;

#[cfg(any(test, target_os = "linux"))]
pub(crate) mod linux;

#[cfg(target_os = "linux")]
pub(crate) fn configure_tun(config: &TunConfig) -> Result<TunStatus> {
    linux::configure_tun(config)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn configure_tun(config: &TunConfig) -> Result<TunStatus> {
    let _ = config;
    Err(crate::tun::unsupported_platform_error())
}
