#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::tun::{TunConfig, TunStatus};
use std::process::Command;
use tfscale_net::{BackendError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl PlannedCommand {
    fn new(program: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

pub(crate) struct LinuxTunDevice {
    interface_name: String,
    cleanup_commands: Vec<PlannedCommand>,
    #[cfg(target_os = "linux")]
    device: tun_rs::SyncDevice,
}

impl LinuxTunDevice {
    pub(crate) fn status(&self) -> TunStatus {
        TunStatus::configured(self.interface_name.clone())
    }

    pub(crate) fn read_packet(&self, buffer: &mut [u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            self.device
                .recv(buffer)
                .map_err(|error| BackendError::CommandFailed(error.to_string()))
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = buffer;
            Err(BackendError::UnsupportedPlatform(format!(
                "Linux TUN read cannot run on {}",
                std::env::consts::OS
            )))
        }
    }

    pub(crate) fn try_read_packet(&self, buffer: &mut [u8]) -> Result<Option<usize>> {
        #[cfg(target_os = "linux")]
        {
            match self.device.recv(buffer) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                Err(error) => Err(BackendError::CommandFailed(error.to_string())),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = buffer;
            Err(BackendError::UnsupportedPlatform(format!(
                "Linux TUN read cannot run on {}",
                std::env::consts::OS
            )))
        }
    }

    pub(crate) fn write_packet(&self, packet: &[u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            self.device
                .send(packet)
                .map_err(|error| BackendError::CommandFailed(error.to_string()))
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = packet;
            Err(BackendError::UnsupportedPlatform(format!(
                "Linux TUN write cannot run on {}",
                std::env::consts::OS
            )))
        }
    }

    pub(crate) fn shutdown(self) -> Result<()> {
        for command in self.cleanup_commands {
            let _ = run_command(&command);
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn configure_tun(config: &TunConfig) -> Result<crate::tun::PlatformTunDevice> {
    let device = configure_tun_device(config)?;
    for command in plan_ip_commands(config) {
        run_command(&command)?;
    }

    Ok(crate::tun::PlatformTunDevice {
        inner: LinuxTunDevice {
            interface_name: config.interface_name.clone(),
            cleanup_commands: plan_cleanup_commands(config),
            #[cfg(target_os = "linux")]
            device,
        },
    })
}

#[cfg(target_os = "linux")]
fn configure_tun_device(config: &TunConfig) -> Result<tun_rs::SyncDevice> {
    if !std::path::Path::new("/dev/net/tun").exists() {
        return Err(BackendError::MissingCommand(
            "missing Linux TUN device: /dev/net/tun".to_string(),
        ));
    }

    let device = tun_rs::DeviceBuilder::new()
        .name(&config.interface_name)
        .ipv4(config.overlay_ip, 32, None)
        .enable(false)
        .build_sync()
        .map_err(|error| {
            BackendError::CommandFailed(format!("failed to create TUN device: {error}"))
        })?;
    device
        .set_nonblocking(true)
        .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
    Ok(device)
}

pub(crate) fn plan_ip_commands(config: &TunConfig) -> Vec<PlannedCommand> {
    vec![
        PlannedCommand::new(
            "ip",
            [
                "addr".to_string(),
                "replace".to_string(),
                format!("{}/32", config.overlay_ip),
                "dev".to_string(),
                config.interface_name.clone(),
            ],
        ),
        PlannedCommand::new(
            "ip",
            [
                "link".to_string(),
                "set".to_string(),
                config.interface_name.clone(),
                "up".to_string(),
            ],
        ),
        PlannedCommand::new(
            "ip",
            [
                "route".to_string(),
                "replace".to_string(),
                config.overlay_cidr.clone(),
                "dev".to_string(),
                config.interface_name.clone(),
            ],
        ),
    ]
}

pub(crate) fn plan_cleanup_commands(config: &TunConfig) -> Vec<PlannedCommand> {
    vec![
        PlannedCommand::new(
            "ip",
            [
                "route".to_string(),
                "del".to_string(),
                config.overlay_cidr.clone(),
                "dev".to_string(),
                config.interface_name.clone(),
            ],
        ),
        PlannedCommand::new(
            "ip",
            [
                "link".to_string(),
                "del".to_string(),
                config.interface_name.clone(),
            ],
        ),
    ]
}

fn run_command(command: &PlannedCommand) -> Result<()> {
    let output = Command::new(&command.program)
        .args(&command.args)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                BackendError::MissingCommand(command.program.clone())
            } else {
                BackendError::CommandFailed(error.to_string())
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(BackendError::CommandFailed(format!(
            "{} {} failed{}{}",
            command.program,
            command.args.join(" "),
            if stderr.is_empty() { "" } else { ": " },
            stderr
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn plans_linux_ip_commands() {
        let commands = plan_ip_commands(&TunConfig {
            interface_name: "tfscale0".to_string(),
            overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
            listen_port: 51820,
            overlay_cidr: "100.64.0.0/10".to_string(),
        });

        assert_eq!(
            commands,
            vec![
                PlannedCommand::new(
                    "ip",
                    ["addr", "replace", "100.64.0.2/32", "dev", "tfscale0"]
                ),
                PlannedCommand::new("ip", ["link", "set", "tfscale0", "up"]),
                PlannedCommand::new(
                    "ip",
                    ["route", "replace", "100.64.0.0/10", "dev", "tfscale0"]
                ),
            ]
        );
    }

    #[test]
    fn plans_linux_cleanup_commands() {
        let commands = plan_cleanup_commands(&TunConfig {
            interface_name: "tfscale0".to_string(),
            overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
            listen_port: 51820,
            overlay_cidr: "100.64.0.0/10".to_string(),
        });

        assert_eq!(
            commands,
            vec![
                PlannedCommand::new("ip", ["route", "del", "100.64.0.0/10", "dev", "tfscale0"]),
                PlannedCommand::new("ip", ["link", "del", "tfscale0"]),
            ]
        );
    }
}
