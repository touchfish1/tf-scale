#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

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

pub(crate) struct MacosTunDevice {
    interface_name: String,
    cleanup_commands: Vec<PlannedCommand>,
    #[cfg(target_os = "macos")]
    device: tun_rs::SyncDevice,
}

impl MacosTunDevice {
    pub(crate) fn status(&self) -> TunStatus {
        TunStatus::configured(self.interface_name.clone())
    }

    pub(crate) fn read_packet(&self, buffer: &mut [u8]) -> Result<usize> {
        #[cfg(target_os = "macos")]
        {
            self.device
                .recv(buffer)
                .map_err(|error| BackendError::CommandFailed(error.to_string()))
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = buffer;
            Err(BackendError::UnsupportedPlatform(format!(
                "macOS TUN read cannot run on {}",
                std::env::consts::OS
            )))
        }
    }

    pub(crate) fn try_read_packet(&self, buffer: &mut [u8]) -> Result<Option<usize>> {
        #[cfg(target_os = "macos")]
        {
            match self.device.recv(buffer) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                Err(error) => Err(BackendError::CommandFailed(error.to_string())),
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = buffer;
            Err(BackendError::UnsupportedPlatform(format!(
                "macOS TUN read cannot run on {}",
                std::env::consts::OS
            )))
        }
    }

    pub(crate) fn write_packet(&self, packet: &[u8]) -> Result<usize> {
        #[cfg(target_os = "macos")]
        {
            self.device
                .send(packet)
                .map_err(|error| BackendError::CommandFailed(error.to_string()))
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = packet;
            Err(BackendError::UnsupportedPlatform(format!(
                "macOS TUN write cannot run on {}",
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

#[cfg(target_os = "macos")]
pub(crate) fn configure_tun(config: &TunConfig) -> Result<crate::tun::PlatformTunDevice> {
    let device = configure_tun_device(config)?;
    let interface_name = device
        .name()
        .map_err(|error| BackendError::CommandFailed(error.to_string()))?;

    for command in plan_ifconfig_commands(config, &interface_name) {
        run_command(&command)?;
    }
    for command in plan_route_commands(config, &interface_name) {
        run_command(&command)?;
    }

    Ok(crate::tun::PlatformTunDevice {
        inner: MacosTunDevice {
            cleanup_commands: plan_cleanup_commands(config, &interface_name),
            interface_name,
            #[cfg(target_os = "macos")]
            device,
        },
    })
}

#[cfg(target_os = "macos")]
fn configure_tun_device(config: &TunConfig) -> Result<tun_rs::SyncDevice> {
    let mut builder = tun_rs::DeviceBuilder::new()
        .associate_route(false)
        .enable(false);

    if let Some(name) = requested_utun_name(config) {
        builder = builder.name(name);
    }

    let device = builder.build_sync().map_err(|error| {
        BackendError::CommandFailed(format!("failed to create utun device: {error}"))
    })?;
    device
        .set_nonblocking(true)
        .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
    Ok(device)
}

fn requested_utun_name(config: &TunConfig) -> Option<&str> {
    config
        .interface_name
        .starts_with("utun")
        .then_some(config.interface_name.as_str())
}

pub(crate) fn plan_ifconfig_commands(
    config: &TunConfig,
    interface_name: &str,
) -> Vec<PlannedCommand> {
    vec![PlannedCommand::new(
        "ifconfig",
        [
            interface_name.to_string(),
            "inet".to_string(),
            config.overlay_ip.to_string(),
            config.overlay_ip.to_string(),
            "netmask".to_string(),
            "255.255.255.255".to_string(),
            "up".to_string(),
        ],
    )]
}

pub(crate) fn plan_route_commands(config: &TunConfig, interface_name: &str) -> Vec<PlannedCommand> {
    vec![PlannedCommand::new(
        "route",
        [
            "-n".to_string(),
            "add".to_string(),
            "-net".to_string(),
            config.overlay_cidr.clone(),
            "-interface".to_string(),
            interface_name.to_string(),
        ],
    )]
}

pub(crate) fn plan_cleanup_commands(
    config: &TunConfig,
    interface_name: &str,
) -> Vec<PlannedCommand> {
    vec![
        PlannedCommand::new(
            "route",
            [
                "-n".to_string(),
                "delete".to_string(),
                "-net".to_string(),
                config.overlay_cidr.clone(),
                "-interface".to_string(),
                interface_name.to_string(),
            ],
        ),
        PlannedCommand::new("ifconfig", [interface_name.to_string(), "down".to_string()]),
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

    fn test_config(interface_name: &str) -> TunConfig {
        TunConfig {
            interface_name: interface_name.to_string(),
            overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
            listen_port: 51820,
            overlay_cidr: "100.64.0.0/10".to_string(),
        }
    }

    #[test]
    fn plans_macos_ifconfig_commands_with_actual_utun_name() {
        let commands = plan_ifconfig_commands(&test_config("tfscale0"), "utun4");

        assert_eq!(
            commands,
            vec![PlannedCommand::new(
                "ifconfig",
                [
                    "utun4",
                    "inet",
                    "100.64.0.2",
                    "100.64.0.2",
                    "netmask",
                    "255.255.255.255",
                    "up",
                ],
            )]
        );
    }

    #[test]
    fn plans_macos_route_commands_with_actual_utun_name() {
        let commands = plan_route_commands(&test_config("tfscale0"), "utun4");

        assert_eq!(
            commands,
            vec![PlannedCommand::new(
                "route",
                ["-n", "add", "-net", "100.64.0.0/10", "-interface", "utun4",],
            )]
        );
    }

    #[test]
    fn plans_macos_cleanup_commands_with_actual_utun_name() {
        let commands = plan_cleanup_commands(&test_config("tfscale0"), "utun4");

        assert_eq!(
            commands,
            vec![
                PlannedCommand::new(
                    "route",
                    [
                        "-n",
                        "delete",
                        "-net",
                        "100.64.0.0/10",
                        "-interface",
                        "utun4",
                    ],
                ),
                PlannedCommand::new("ifconfig", ["utun4", "down"]),
            ]
        );
    }

    #[test]
    fn only_explicit_utun_names_are_requested() {
        let logical_config = test_config("tfscale0");
        let explicit_config = test_config("utun12");

        assert_eq!(requested_utun_name(&logical_config), None);
        assert_eq!(requested_utun_name(&explicit_config), Some("utun12"));
    }
}
