use anyhow::{Context, Result};
use serde::Serialize;
use std::{fs, path::PathBuf};

const DEFAULT_SUFFIX: &str = "mesh";
const LINUX_RESOLVED_CONF: &str = "/etc/systemd/resolved.conf.d/tfscale-magicdns.conf";
#[cfg(any(test, target_os = "macos"))]
const MACOS_RESOLVER_DIR: &str = "/etc/resolver";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolverConfig {
    pub suffix: String,
    pub nameserver: String,
    pub port: u16,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            suffix: DEFAULT_SUFFIX.to_string(),
            nameserver: "127.0.0.1".to_string(),
            port: 1053,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolverPlan {
    pub platform: ResolverPlatform,
    pub config_path: PathBuf,
    pub install_content: String,
    pub reload_commands: Vec<Vec<String>>,
    pub uninstall_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolverStatus {
    pub platform: String,
    pub config_path: PathBuf,
    pub installed: bool,
    pub content_matches: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolverPlatform {
    LinuxSystemdResolved,
    #[cfg(any(test, target_os = "macos"))]
    MacosResolver,
}

pub fn linux_systemd_resolved_plan(config: &ResolverConfig) -> ResolverPlan {
    ResolverPlan {
        platform: ResolverPlatform::LinuxSystemdResolved,
        config_path: PathBuf::from(LINUX_RESOLVED_CONF),
        install_content: format!(
            "[Resolve]\nDNS={}:{}\nDomains=~{}\n",
            config.nameserver, config.port, config.suffix
        ),
        reload_commands: vec![vec![
            "systemctl".to_string(),
            "reload".to_string(),
            "systemd-resolved".to_string(),
        ]],
        uninstall_paths: vec![PathBuf::from(LINUX_RESOLVED_CONF)],
    }
}

pub fn install(plan: &ResolverPlan) -> Result<()> {
    if let Some(parent) = plan.config_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create resolver config dir {}", parent.display())
        })?;
    }
    fs::write(&plan.config_path, plan.install_content.as_bytes()).with_context(|| {
        format!(
            "failed to write resolver config {}",
            plan.config_path.display()
        )
    })?;
    run_reload_commands(plan)?;
    Ok(())
}

pub fn uninstall(plan: &ResolverPlan) -> Result<()> {
    for path in &plan.uninstall_paths {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to remove resolver config {}", path.display())
                });
            }
        }
    }
    run_reload_commands(plan)?;
    Ok(())
}

pub fn status(plan: &ResolverPlan) -> ResolverStatus {
    match fs::read_to_string(&plan.config_path) {
        Ok(content) => ResolverStatus {
            platform: platform_label(&plan.platform).to_string(),
            config_path: plan.config_path.clone(),
            installed: true,
            content_matches: content == plan.install_content,
            message: if content == plan.install_content {
                Some("resolver config is installed".to_string())
            } else {
                Some("resolver config exists but does not match tf-scale plan".to_string())
            },
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ResolverStatus {
            platform: platform_label(&plan.platform).to_string(),
            config_path: plan.config_path.clone(),
            installed: false,
            content_matches: false,
            message: Some("resolver config is not installed".to_string()),
        },
        Err(error) => ResolverStatus {
            platform: platform_label(&plan.platform).to_string(),
            config_path: plan.config_path.clone(),
            installed: false,
            content_matches: false,
            message: Some(format!("failed to read resolver config: {error}")),
        },
    }
}

pub fn platform_label(platform: &ResolverPlatform) -> &'static str {
    match platform {
        ResolverPlatform::LinuxSystemdResolved => "linux-systemd-resolved",
        #[cfg(any(test, target_os = "macos"))]
        ResolverPlatform::MacosResolver => "macos-resolver",
    }
}

fn run_reload_commands(plan: &ResolverPlan) -> Result<()> {
    for command in &plan.reload_commands {
        let Some((program, args)) = command.split_first() else {
            continue;
        };
        let status = std::process::Command::new(program)
            .args(args)
            .status()
            .with_context(|| {
                format!(
                    "failed to run resolver reload command: {}",
                    command.join(" ")
                )
            })?;
        if !status.success() {
            anyhow::bail!(
                "resolver reload command failed with status {}: {}",
                status,
                command.join(" ")
            );
        }
    }
    Ok(())
}

#[cfg(any(test, target_os = "macos"))]
pub fn macos_resolver_plan(config: &ResolverConfig) -> ResolverPlan {
    let config_path = PathBuf::from(MACOS_RESOLVER_DIR).join(&config.suffix);
    ResolverPlan {
        platform: ResolverPlatform::MacosResolver,
        config_path: config_path.clone(),
        install_content: format!("nameserver {}\nport {}\n", config.nameserver, config.port),
        reload_commands: Vec::new(),
        uninstall_paths: vec![config_path],
    }
}

#[cfg(target_os = "linux")]
pub fn current_platform_plan(config: &ResolverConfig) -> ResolverPlan {
    linux_systemd_resolved_plan(config)
}

#[cfg(target_os = "macos")]
pub fn current_platform_plan(config: &ResolverConfig) -> ResolverPlan {
    macos_resolver_plan(config)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn current_platform_plan(config: &ResolverConfig) -> ResolverPlan {
    linux_systemd_resolved_plan(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_linux_systemd_resolved_plan() {
        let plan = linux_systemd_resolved_plan(&ResolverConfig::default());

        assert_eq!(plan.platform, ResolverPlatform::LinuxSystemdResolved);
        assert_eq!(plan.config_path, PathBuf::from(LINUX_RESOLVED_CONF));
        assert!(plan.install_content.contains("DNS=127.0.0.1:1053"));
        assert!(plan.install_content.contains("Domains=~mesh"));
        assert_eq!(
            plan.reload_commands,
            vec![vec![
                "systemctl".to_string(),
                "reload".to_string(),
                "systemd-resolved".to_string()
            ]]
        );
    }

    #[test]
    fn builds_macos_resolver_plan() {
        let plan = macos_resolver_plan(&ResolverConfig::default());

        assert_eq!(plan.platform, ResolverPlatform::MacosResolver);
        assert_eq!(plan.config_path, PathBuf::from("/etc/resolver/mesh"));
        assert_eq!(plan.install_content, "nameserver 127.0.0.1\nport 1053\n");
        assert!(plan.reload_commands.is_empty());
    }

    #[test]
    fn status_reports_matching_installed_config() {
        let temp_dir =
            std::env::temp_dir().join(format!("tfscale-resolver-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("temp dir");
        let plan = ResolverPlan {
            platform: ResolverPlatform::LinuxSystemdResolved,
            config_path: temp_dir.join("tfscale-magicdns.conf"),
            install_content: "[Resolve]\nDNS=127.0.0.1:1053\nDomains=~mesh\n".to_string(),
            reload_commands: Vec::new(),
            uninstall_paths: vec![temp_dir.join("tfscale-magicdns.conf")],
        };

        install(&plan).expect("install resolver config");
        let status = status(&plan);
        uninstall(&plan).expect("uninstall resolver config");
        let _ = fs::remove_dir_all(&temp_dir);

        assert!(status.installed);
        assert!(status.content_matches);
    }
}
