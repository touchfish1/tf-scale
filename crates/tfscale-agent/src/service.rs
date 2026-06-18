use anyhow::{Context, Result};
use serde::Serialize;
use std::{fs, net::SocketAddr, path::PathBuf, process::Command};

const SERVICE_NAME: &str = "tfscale-agent";
const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/tfscale-agent.service";
const SYSTEMD_ENV_PATH: &str = "/etc/tfscale/agent.env";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceConfig {
    pub binary_path: PathBuf,
    pub state_dir: PathBuf,
    pub control_url: String,
    pub login_key: String,
    pub dns_listen: SocketAddr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServicePlan {
    pub platform: ServicePlatform,
    pub unit_path: PathBuf,
    pub unit_content: String,
    pub env_path: PathBuf,
    pub env_content: String,
    pub install_commands: Vec<Vec<String>>,
    pub uninstall_commands: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServicePlatform {
    LinuxSystemd,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServiceStatus {
    pub platform: String,
    pub unit_path: PathBuf,
    pub env_path: PathBuf,
    pub installed: bool,
    pub env_installed: bool,
    pub enabled: Option<bool>,
    pub active: Option<bool>,
    pub message: Option<String>,
}

pub fn current_platform_plan(config: &ServiceConfig) -> ServicePlan {
    #[cfg(target_os = "linux")]
    {
        linux_systemd_plan(config)
    }

    #[cfg(not(target_os = "linux"))]
    {
        unsupported_plan(config)
    }
}

pub fn linux_systemd_plan(config: &ServiceConfig) -> ServicePlan {
    ServicePlan {
        platform: ServicePlatform::LinuxSystemd,
        unit_path: PathBuf::from(SYSTEMD_UNIT_PATH),
        unit_content: linux_systemd_unit(config),
        env_path: PathBuf::from(SYSTEMD_ENV_PATH),
        env_content: linux_systemd_env(config),
        install_commands: vec![
            vec!["systemctl".to_string(), "daemon-reload".to_string()],
            vec![
                "systemctl".to_string(),
                "enable".to_string(),
                SERVICE_NAME.to_string(),
            ],
        ],
        uninstall_commands: vec![
            vec![
                "systemctl".to_string(),
                "disable".to_string(),
                "--now".to_string(),
                SERVICE_NAME.to_string(),
            ],
            vec!["systemctl".to_string(), "daemon-reload".to_string()],
        ],
    }
}

pub fn unsupported_plan(_config: &ServiceConfig) -> ServicePlan {
    ServicePlan {
        platform: ServicePlatform::Unsupported,
        unit_path: PathBuf::from(SYSTEMD_UNIT_PATH),
        unit_content: String::new(),
        env_path: PathBuf::from(SYSTEMD_ENV_PATH),
        env_content: String::new(),
        install_commands: Vec::new(),
        uninstall_commands: Vec::new(),
    }
}

pub fn install(plan: &ServicePlan) -> Result<()> {
    ensure_supported(plan)?;
    if let Some(parent) = plan.unit_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create service dir {}", parent.display()))?;
    }
    if let Some(parent) = plan.env_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create service env dir {}", parent.display()))?;
    }
    fs::write(&plan.unit_path, plan.unit_content.as_bytes())
        .with_context(|| format!("failed to write service unit {}", plan.unit_path.display()))?;
    write_env_file(plan)?;
    run_commands(&plan.install_commands)?;
    Ok(())
}

pub fn uninstall(plan: &ServicePlan) -> Result<()> {
    ensure_supported(plan)?;
    let _ = run_commands(&plan.uninstall_commands);
    match fs::remove_file(&plan.unit_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to remove service unit {}", plan.unit_path.display())
            });
        }
    }
    match fs::remove_file(&plan.env_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to remove service env file {}",
                    plan.env_path.display()
                )
            });
        }
    }
    run_commands(&[vec!["systemctl".to_string(), "daemon-reload".to_string()]])?;
    Ok(())
}

pub fn status(plan: &ServicePlan) -> ServiceStatus {
    let installed = plan.unit_path.exists();
    let env_installed = plan.env_path.exists();
    let (enabled, enabled_message) = systemctl_bool("is-enabled");
    let (active, active_message) = systemctl_bool("is-active");
    let message = enabled_message.or(active_message).or_else(|| {
        if installed {
            Some("service unit is installed".to_string())
        } else {
            Some("service unit is not installed".to_string())
        }
    });

    ServiceStatus {
        platform: platform_label(&plan.platform).to_string(),
        unit_path: plan.unit_path.clone(),
        env_path: plan.env_path.clone(),
        installed,
        env_installed,
        enabled,
        active,
        message,
    }
}

pub fn platform_label(platform: &ServicePlatform) -> &'static str {
    match platform {
        ServicePlatform::LinuxSystemd => "linux-systemd",
        ServicePlatform::Unsupported => "unsupported",
    }
}

fn linux_systemd_unit(config: &ServiceConfig) -> String {
    let binary = escape_systemd_arg(&config.binary_path.display().to_string());
    let state_dir = escape_systemd_arg(&config.state_dir.display().to_string());
    let dns_listen = escape_systemd_arg(&config.dns_listen.to_string());
    let env_path = escape_systemd_arg(SYSTEMD_ENV_PATH);

    format!(
        "[Unit]\n\
         Description=tf-scale agent\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         EnvironmentFile={env_path}\n\
         ExecStart={binary} --state-dir {state_dir} up --login-key ${{TFSCALE_LOGIN_KEY}} --control-url ${{TFSCALE_CONTROL_URL}} --dns-listen {dns_listen}\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

fn linux_systemd_env(config: &ServiceConfig) -> String {
    format!(
        "TFSCALE_LOGIN_KEY={}\nTFSCALE_CONTROL_URL={}\n",
        systemd_env_value(&config.login_key),
        systemd_env_value(&config.control_url)
    )
}

fn escape_systemd_arg(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | ':' | '.' | '_' | '-' | '='))
    {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

fn systemd_env_value(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | ':' | '.' | '_' | '-' | '='))
    {
        return value.to_string();
    }

    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn write_env_file(plan: &ServicePlan) -> Result<()> {
    fs::write(&plan.env_path, plan.env_content.as_bytes()).with_context(|| {
        format!(
            "failed to write service env file {}",
            plan.env_path.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&plan.env_path, fs::Permissions::from_mode(0o600)).with_context(
            || {
                format!(
                    "failed to set service env file permissions {}",
                    plan.env_path.display()
                )
            },
        )?;
    }

    Ok(())
}

fn ensure_supported(plan: &ServicePlan) -> Result<()> {
    if plan.platform == ServicePlatform::Unsupported {
        anyhow::bail!("service install is not supported on this platform yet");
    }
    Ok(())
}

fn run_commands(commands: &[Vec<String>]) -> Result<()> {
    for command in commands {
        let Some((program, args)) = command.split_first() else {
            continue;
        };
        let status = Command::new(program)
            .args(args)
            .status()
            .with_context(|| format!("failed to run service command: {}", command.join(" ")))?;
        if !status.success() {
            anyhow::bail!(
                "service command failed with status {}: {}",
                status,
                command.join(" ")
            );
        }
    }
    Ok(())
}

fn systemctl_bool(command: &str) -> (Option<bool>, Option<String>) {
    let output = Command::new("systemctl")
        .arg(command)
        .arg(SERVICE_NAME)
        .output();
    let Ok(output) = output else {
        return (None, Some(format!("systemctl {command} is not available")));
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = stdout.trim();
    match (command, value) {
        ("is-enabled", "enabled") => (Some(true), None),
        ("is-enabled", "disabled" | "static" | "indirect") => (Some(false), None),
        ("is-active", "active") => (Some(true), None),
        ("is-active", "inactive" | "failed" | "activating" | "deactivating") => (Some(false), None),
        _ if output.status.success() => (Some(true), None),
        _ => (Some(false), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_linux_systemd_unit() {
        let config = test_config();
        let plan = linux_systemd_plan(&config);

        assert_eq!(plan.platform, ServicePlatform::LinuxSystemd);
        assert_eq!(plan.unit_path, PathBuf::from(SYSTEMD_UNIT_PATH));
        assert_eq!(plan.env_path, PathBuf::from(SYSTEMD_ENV_PATH));
        assert!(plan.unit_content.contains("Description=tf-scale agent"));
        assert!(
            plan.unit_content
                .contains("EnvironmentFile=/etc/tfscale/agent.env")
        );
        assert!(plan.unit_content.contains("--state-dir /var/lib/tfscale"));
        assert!(
            plan.unit_content
                .contains("--login-key ${TFSCALE_LOGIN_KEY}")
        );
        assert!(
            plan.unit_content
                .contains("--control-url ${TFSCALE_CONTROL_URL}")
        );
        assert!(plan.unit_content.contains("--dns-listen 127.0.0.1:1053"));
        assert!(plan.env_content.contains("TFSCALE_LOGIN_KEY=tfkey_test"));
        assert!(
            plan.env_content
                .contains("TFSCALE_CONTROL_URL=http://control:8080")
        );
        assert!(
            plan.install_commands
                .iter()
                .any(|command| command == &["systemctl", "enable", SERVICE_NAME])
        );
    }

    #[test]
    fn quotes_systemd_arguments_with_spaces() {
        assert_eq!(
            escape_systemd_arg("/opt/tf scale/agent"),
            "'/opt/tf scale/agent'"
        );
    }

    #[test]
    fn quotes_systemd_env_values_with_spaces() {
        assert_eq!(systemd_env_value("tf key"), "\"tf key\"");
    }

    fn test_config() -> ServiceConfig {
        ServiceConfig {
            binary_path: PathBuf::from("/usr/local/bin/tfscale-agent"),
            state_dir: PathBuf::from("/var/lib/tfscale"),
            control_url: "http://control:8080".to_string(),
            login_key: "tfkey_test".to_string(),
            dns_listen: "127.0.0.1:1053".parse().expect("dns listen"),
        }
    }
}
