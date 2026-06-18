use std::path::PathBuf;

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
}
