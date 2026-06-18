mod dns;
mod resolver;
mod service;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};
use tfscale_core::DeviceId;
use tfscale_core::protocol::{
    BackendStatusPayload, DnsRecord, EndpointPayload, EndpointProbeRequest, EndpointProbeResponse,
    HeartbeatRequest, NetworkMapPeer, NetworkMapResponse, RegisterDeviceRequest,
    RegisterDeviceResponse,
};
use tfscale_custom::CustomBackend;
use tfscale_net::{
    BackendCredential, Endpoint, EndpointKind, LocalBackendConfig, NetworkBackend, PeerConfig,
    PeerPathDiagnosticKind, RelayConfig, TransportProtocol,
};
use tracing::{info, warn};
use uuid::Uuid;

const DEFAULT_INTERFACE_NAME: &str = "tfscale0";
const DEFAULT_LISTEN_PORT: u16 = 51820;
const DEFAULT_DNS_LISTEN: &str = "127.0.0.1:1053";
const DEFAULT_DNS_SUFFIX: &str = "mesh";
const DIRECT_PATH_MAINTENANCE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Parser)]
#[command(name = "tfscale-agent", version, about = "tf-scale node agent")]
struct Cli {
    #[arg(long, env = "TFSCALE_STATE_DIR")]
    state_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Up {
        #[arg(long = "login-key")]
        login_key: String,

        #[arg(long, default_value = "http://127.0.0.1:8080")]
        control_url: String,

        #[arg(long, default_value = DEFAULT_DNS_LISTEN)]
        dns_listen: SocketAddr,
    },
    Down,
    Dns {
        #[command(subcommand)]
        command: DnsCommand,
    },
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum DnsCommand {
    Plan,
    Install,
    Uninstall,
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    Plan {
        #[arg(long = "login-key")]
        login_key: String,

        #[arg(long, default_value = "http://127.0.0.1:8080")]
        control_url: String,

        #[arg(long, default_value = DEFAULT_DNS_LISTEN)]
        dns_listen: SocketAddr,
    },
    Install {
        #[arg(long = "login-key")]
        login_key: String,

        #[arg(long, default_value = "http://127.0.0.1:8080")]
        control_url: String,

        #[arg(long, default_value = DEFAULT_DNS_LISTEN)]
        dns_listen: SocketAddr,
    },
    Uninstall,
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let state_dir = cli.state_dir.unwrap_or_else(default_state_dir);

    match cli.command {
        Command::Up {
            login_key,
            control_url,
            dns_listen,
        } => {
            info!(%control_url, "agent up requested");
            agent_up(&state_dir, &control_url, &login_key, dns_listen).await?;
        }
        Command::Down => {
            let backend = CustomBackend::with_state_dir(DEFAULT_INTERFACE_NAME, &state_dir);
            backend.shutdown().await?;
            println!("agent backend stopped");
        }
        Command::Dns { command } => {
            let state = AgentState::load(&state_dir)?.unwrap_or_default();
            match command {
                DnsCommand::Plan => print_dns_resolver_plan(&state),
                DnsCommand::Install => install_dns_resolver(&state)?,
                DnsCommand::Uninstall => uninstall_dns_resolver(&state)?,
                DnsCommand::Status { json } => print_dns_resolver_status(&state, json)?,
            }
        }
        Command::Service { command } => match command {
            ServiceCommand::Plan {
                login_key,
                control_url,
                dns_listen,
            } => print_service_plan(&state_dir, &control_url, &login_key, dns_listen)?,
            ServiceCommand::Install {
                login_key,
                control_url,
                dns_listen,
            } => install_service(&state_dir, &control_url, &login_key, dns_listen)?,
            ServiceCommand::Uninstall => uninstall_service()?,
            ServiceCommand::Status { json } => print_service_status(json)?,
        },
        Command::Doctor { json } => {
            let backend = CustomBackend::with_state_dir(DEFAULT_INTERFACE_NAME, &state_dir);
            let status = backend.status().await?;
            let state = AgentState::load(&state_dir)?.unwrap_or_default();
            let report = AgentDoctorReport::from_state_and_status(&state, &status);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_doctor_report(&report);
            }
        }
        Command::Status { json } => {
            let backend = CustomBackend::with_state_dir(DEFAULT_INTERFACE_NAME, &state_dir);
            let status = backend.status().await?;
            let state = AgentState::load(&state_dir)?.unwrap_or_default();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&AgentStatusOutput::from_state_and_status(
                        state, status,
                    ))?
                );
            } else {
                println!(
                    "device={} ipv4={} backend={} interface={} healthy={} message={}",
                    state.device_id.as_deref().unwrap_or("-"),
                    state.ipv4.as_deref().unwrap_or("-"),
                    status.backend_type.as_str(),
                    status.interface_name,
                    status.healthy,
                    status.message.unwrap_or_default()
                );
            }
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct AgentStatusOutput {
    device_id: Option<String>,
    network_id: Option<String>,
    ipv4: Option<String>,
    dns_records: Vec<DnsRecord>,
    dns: DnsStatusOutput,
    backend_public_credential_present: bool,
    backend: BackendStatusOutput,
}

#[derive(Debug, Serialize)]
struct DnsStatusOutput {
    enabled: bool,
    listen: String,
    healthy: bool,
    records: usize,
    message: Option<String>,
    resolver: resolver::ResolverStatus,
}

#[derive(Debug, Serialize)]
struct BackendStatusOutput {
    backend_type: String,
    interface_name: String,
    healthy: bool,
    message: Option<String>,
    peers: Vec<tfscale_net::PeerPathDiagnostic>,
}

impl AgentStatusOutput {
    fn from_state_and_status(state: AgentState, status: tfscale_net::BackendStatus) -> Self {
        let resolver_status = resolver::status(&dns_resolver_plan(&state));
        let dns_records = state.dns_records;
        let dns_record_count = dns_records.len();
        Self {
            device_id: state.device_id,
            network_id: state.network_id,
            ipv4: state.ipv4,
            dns_records,
            dns: DnsStatusOutput {
                enabled: state.dns_enabled,
                listen: state.dns_listen.clone(),
                healthy: state.dns_healthy,
                records: dns_record_count,
                message: state.dns_message,
                resolver: resolver_status,
            },
            backend_public_credential_present: !state.backend_public_credential.is_empty(),
            backend: BackendStatusOutput {
                backend_type: status.backend_type.as_str().to_string(),
                interface_name: status.interface_name,
                healthy: status.healthy,
                message: status.message,
                peers: status.peers,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DoctorLevel {
    Ok,
    Warn,
    Fail,
}

impl DoctorLevel {
    fn label(&self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct DoctorCheck {
    id: String,
    level: DoctorLevel,
    summary: String,
    detail: Option<String>,
    suggestion: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct AgentDoctorReport {
    overall: DoctorLevel,
    checks: Vec<DoctorCheck>,
    next_steps: Vec<String>,
}

impl AgentDoctorReport {
    fn from_state_and_status(state: &AgentState, status: &tfscale_net::BackendStatus) -> Self {
        let mut checks = Vec::new();
        checks.push(check_registered(state));
        checks.push(check_backend(status));
        checks.push(check_backend_peers(status));
        checks.push(check_dns_listener(state));
        checks.push(check_dns_snapshot(state));
        checks.push(check_dns_system_resolver(state));

        let overall = overall_level(&checks);
        let next_steps = next_steps(&checks);

        Self {
            overall,
            checks,
            next_steps,
        }
    }
}

fn check_registered(state: &AgentState) -> DoctorCheck {
    let registered = state.device_id.is_some()
        && state.node_key.is_some()
        && state.network_id.is_some()
        && state.ipv4.is_some();
    if registered {
        DoctorCheck {
            id: "state.registered".to_string(),
            level: DoctorLevel::Ok,
            summary: format!(
                "{} {}",
                state.device_id.as_deref().unwrap_or("-"),
                state.ipv4.as_deref().unwrap_or("-")
            ),
            detail: state.network_id.clone(),
            suggestion: None,
        }
    } else {
        DoctorCheck {
            id: "state.registered".to_string(),
            level: DoctorLevel::Fail,
            summary: "agent is not registered".to_string(),
            detail: None,
            suggestion: Some("run tfscale-agent up --login-key <key>".to_string()),
        }
    }
}

fn check_backend(status: &tfscale_net::BackendStatus) -> DoctorCheck {
    DoctorCheck {
        id: "backend.healthy".to_string(),
        level: if status.healthy {
            DoctorLevel::Ok
        } else {
            DoctorLevel::Fail
        },
        summary: format!("{} {}", status.backend_type.as_str(), status.interface_name),
        detail: status.message.clone(),
        suggestion: if status.healthy {
            None
        } else {
            Some("check TUN permissions and backend status message".to_string())
        },
    }
}

fn check_backend_peers(status: &tfscale_net::BackendStatus) -> DoctorCheck {
    let direct = status
        .peers
        .iter()
        .filter(|peer| peer.path == PeerPathDiagnosticKind::Direct)
        .count();
    let relay = status
        .peers
        .iter()
        .filter(|peer| peer.path == PeerPathDiagnosticKind::Relay)
        .count();
    let unknown = status
        .peers
        .iter()
        .filter(|peer| peer.path == PeerPathDiagnosticKind::Unknown)
        .count();
    let level = if unknown > 0 {
        DoctorLevel::Warn
    } else {
        DoctorLevel::Ok
    };

    DoctorCheck {
        id: "backend.peers".to_string(),
        level,
        summary: format!("direct={direct} relay={relay} unknown={unknown}"),
        detail: Some(format!("total={}", status.peers.len())),
        suggestion: if unknown > 0 {
            Some("wait for endpoint probing or check relay availability".to_string())
        } else {
            None
        },
    }
}

fn check_dns_listener(state: &AgentState) -> DoctorCheck {
    if !state.dns_enabled {
        return DoctorCheck {
            id: "dns.listener".to_string(),
            level: DoctorLevel::Warn,
            summary: "DNS listener has not been started".to_string(),
            detail: Some(format!("listen={}", state.dns_listen)),
            suggestion: Some("run tfscale-agent up to start MagicDNS".to_string()),
        };
    }

    DoctorCheck {
        id: "dns.listener".to_string(),
        level: if state.dns_healthy {
            DoctorLevel::Ok
        } else {
            DoctorLevel::Fail
        },
        summary: format!("{} records={}", state.dns_listen, state.dns_records.len()),
        detail: state.dns_message.clone(),
        suggestion: if state.dns_healthy {
            Some(format!(
                "test with: dig @{} {}.{} A",
                state.dns_listen, "<hostname>", DEFAULT_DNS_SUFFIX
            ))
        } else {
            Some("check whether the DNS port is already in use".to_string())
        },
    }
}

fn check_dns_snapshot(state: &AgentState) -> DoctorCheck {
    if state.dns_records.is_empty() {
        DoctorCheck {
            id: "dns.snapshot".to_string(),
            level: DoctorLevel::Warn,
            summary: "no DNS records in local snapshot".to_string(),
            detail: None,
            suggestion: Some("check tfscalectl dns records and wait for agent sync".to_string()),
        }
    } else {
        DoctorCheck {
            id: "dns.snapshot".to_string(),
            level: DoctorLevel::Ok,
            summary: format!("records={}", state.dns_records.len()),
            detail: state
                .dns_records
                .first()
                .map(|record| format!("example={} -> {}", record.name, record.value)),
            suggestion: None,
        }
    }
}

fn check_dns_system_resolver(state: &AgentState) -> DoctorCheck {
    let plan = dns_resolver_plan(state);
    let status = resolver::status(&plan);

    if status.installed && status.content_matches {
        DoctorCheck {
            id: "dns.system_resolver".to_string(),
            level: DoctorLevel::Ok,
            summary: format!("installed path={}", status.config_path.display()),
            detail: Some(format!("platform={}", status.platform)),
            suggestion: None,
        }
    } else if status.installed {
        DoctorCheck {
            id: "dns.system_resolver".to_string(),
            level: DoctorLevel::Warn,
            summary: format!(
                "installed but content differs path={}",
                status.config_path.display()
            ),
            detail: status.message,
            suggestion: Some(
                "run tfscale-agent dns install to refresh resolver config".to_string(),
            ),
        }
    } else {
        DoctorCheck {
            id: "dns.system_resolver".to_string(),
            level: DoctorLevel::Warn,
            summary: format!(
                "not installed; planned path={}",
                status.config_path.display()
            ),
            detail: Some(format!("platform={}", status.platform)),
            suggestion: Some(
                "run tfscale-agent dns install to enable system hostname resolution".to_string(),
            ),
        }
    }
}

fn overall_level(checks: &[DoctorCheck]) -> DoctorLevel {
    if checks.iter().any(|check| check.level == DoctorLevel::Fail) {
        DoctorLevel::Fail
    } else if checks.iter().any(|check| check.level == DoctorLevel::Warn) {
        DoctorLevel::Warn
    } else {
        DoctorLevel::Ok
    }
}

fn next_steps(checks: &[DoctorCheck]) -> Vec<String> {
    let mut steps = checks
        .iter()
        .filter_map(|check| check.suggestion.clone())
        .collect::<Vec<_>>();
    steps.dedup();
    steps
}

fn print_doctor_report(report: &AgentDoctorReport) {
    println!("tf-scale doctor overall={}", report.overall.label());
    println!();
    for check in &report.checks {
        println!(
            "{:<4} {:<22} {}",
            check.level.label(),
            check.id,
            check.summary
        );
        if let Some(detail) = check.detail.as_deref() {
            println!("     {:<22} {}", "", detail);
        }
    }
    if !report.next_steps.is_empty() {
        println!();
        println!("Next:");
        for step in &report.next_steps {
            println!("  {step}");
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

fn print_dns_resolver_plan(state: &AgentState) {
    let plan = dns_resolver_plan(state);

    println!("platform={}", resolver::platform_label(&plan.platform));
    println!("config_path={}", plan.config_path.display());
    println!("install_content:");
    print!("{}", plan.install_content);
    if !plan.reload_commands.is_empty() {
        println!("reload_commands:");
        for command in plan.reload_commands {
            println!("{}", command.join(" "));
        }
    }
    if !plan.uninstall_paths.is_empty() {
        println!("uninstall_paths:");
        for path in plan.uninstall_paths {
            println!("{}", path.display());
        }
    }
}

fn install_dns_resolver(state: &AgentState) -> Result<()> {
    let plan = dns_resolver_plan(state);
    resolver::install(&plan)?;
    println!(
        "DNS resolver installed: platform={} config_path={}",
        resolver::platform_label(&plan.platform),
        plan.config_path.display()
    );
    Ok(())
}

fn uninstall_dns_resolver(state: &AgentState) -> Result<()> {
    let plan = dns_resolver_plan(state);
    resolver::uninstall(&plan)?;
    println!(
        "DNS resolver uninstalled: platform={} config_path={}",
        resolver::platform_label(&plan.platform),
        plan.config_path.display()
    );
    Ok(())
}

fn print_dns_resolver_status(state: &AgentState, json: bool) -> Result<()> {
    let plan = dns_resolver_plan(state);
    let status = resolver::status(&plan);
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "platform={} installed={} content_matches={} config_path={} message={}",
            status.platform,
            status.installed,
            status.content_matches,
            status.config_path.display(),
            status.message.unwrap_or_default()
        );
    }
    Ok(())
}

fn print_service_plan(
    state_dir: &PathBuf,
    control_url: &str,
    login_key: &str,
    dns_listen: SocketAddr,
) -> Result<()> {
    let plan = service_plan(state_dir, control_url, login_key, dns_listen)?;
    println!("platform={}", service::platform_label(&plan.platform));
    println!("unit_path={}", plan.unit_path.display());
    println!("unit_content:");
    print!("{}", plan.unit_content);
    if !plan.install_commands.is_empty() {
        println!("install_commands:");
        for command in plan.install_commands {
            println!("{}", command.join(" "));
        }
    }
    if !plan.uninstall_commands.is_empty() {
        println!("uninstall_commands:");
        for command in plan.uninstall_commands {
            println!("{}", command.join(" "));
        }
    }
    Ok(())
}

fn install_service(
    state_dir: &PathBuf,
    control_url: &str,
    login_key: &str,
    dns_listen: SocketAddr,
) -> Result<()> {
    let plan = service_plan(state_dir, control_url, login_key, dns_listen)?;
    service::install(&plan)?;
    println!(
        "agent service installed: platform={} unit_path={}",
        service::platform_label(&plan.platform),
        plan.unit_path.display()
    );
    Ok(())
}

fn uninstall_service() -> Result<()> {
    let plan = service::current_platform_plan(&service::ServiceConfig {
        binary_path: current_exe()?,
        state_dir: default_state_dir(),
        control_url: "http://127.0.0.1:8080".to_string(),
        login_key: "unused".to_string(),
        dns_listen: DEFAULT_DNS_LISTEN.parse()?,
    });
    service::uninstall(&plan)?;
    println!(
        "agent service uninstalled: platform={} unit_path={}",
        service::platform_label(&plan.platform),
        plan.unit_path.display()
    );
    Ok(())
}

fn print_service_status(json: bool) -> Result<()> {
    let plan = service::current_platform_plan(&service::ServiceConfig {
        binary_path: current_exe()?,
        state_dir: default_state_dir(),
        control_url: "http://127.0.0.1:8080".to_string(),
        login_key: "unused".to_string(),
        dns_listen: DEFAULT_DNS_LISTEN.parse()?,
    });
    let status = service::status(&plan);
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "platform={} installed={} env_installed={} enabled={} active={} unit_path={} env_path={} message={}",
            status.platform,
            status.installed,
            status.env_installed,
            optional_bool_label(status.enabled),
            optional_bool_label(status.active),
            status.unit_path.display(),
            status.env_path.display(),
            status.message.unwrap_or_default()
        );
    }
    Ok(())
}

fn service_plan(
    state_dir: &PathBuf,
    control_url: &str,
    login_key: &str,
    dns_listen: SocketAddr,
) -> Result<service::ServicePlan> {
    Ok(service::current_platform_plan(&service::ServiceConfig {
        binary_path: current_exe()?,
        state_dir: state_dir.clone(),
        control_url: control_url.to_string(),
        login_key: login_key.to_string(),
        dns_listen,
    }))
}

fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("failed to find current executable path")
}

fn optional_bool_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

fn dns_resolver_plan(state: &AgentState) -> resolver::ResolverPlan {
    let (nameserver, port) = dns_listen_parts(&state.dns_listen);
    let config = resolver::ResolverConfig {
        suffix: DEFAULT_DNS_SUFFIX.to_string(),
        nameserver,
        port,
    };
    resolver::current_platform_plan(&config)
}

fn dns_listen_parts(value: &str) -> (String, u16) {
    value
        .parse::<SocketAddr>()
        .map(|addr| (addr.ip().to_string(), addr.port()))
        .unwrap_or_else(|_| ("127.0.0.1".to_string(), 1053))
}

async fn agent_up(
    state_dir: &PathBuf,
    control_url: &str,
    login_key: &str,
    dns_listen: SocketAddr,
) -> Result<AgentState> {
    let client = reqwest::Client::new();
    let mut state = AgentState::load(state_dir)?.unwrap_or_else(AgentState::new);
    let backend = CustomBackend::with_state_dir(DEFAULT_INTERFACE_NAME, state_dir);
    let dns_runtime = dns::DnsRuntime::new(state.dns_records.clone());
    let _dns_handle = match dns::spawn_dns_proxy(
        dns::DnsConfig {
            listen: dns_listen,
            suffix: DEFAULT_DNS_SUFFIX.to_string(),
        },
        dns_runtime.clone(),
    )
    .await
    {
        Ok(handle) => {
            state.dns_enabled = true;
            state.dns_listen = dns_listen.to_string();
            state.dns_healthy = true;
            state.dns_message = Some("DNS proxy listening".to_string());
            Some(handle)
        }
        Err(error) => {
            state.dns_enabled = true;
            state.dns_listen = dns_listen.to_string();
            state.dns_healthy = false;
            state.dns_message = Some(format!("DNS proxy failed to bind: {error}"));
            warn!(%error, %dns_listen, "DNS proxy failed to start");
            None
        }
    };

    ensure_backend_credentials(&backend, &mut state).await?;
    state.save(state_dir)?;

    register_agent_if_needed(&client, state_dir, control_url, login_key, &mut state).await?;

    println!(
        "agent registered: device={} ipv4={} network={}",
        state.device_id.as_deref().unwrap_or("-"),
        state.ipv4.as_deref().unwrap_or("-"),
        state.network_id.as_deref().unwrap_or("-")
    );

    let mut last_applied_network_map_version = None;
    sync_agent_once(
        &client,
        &backend,
        &dns_runtime,
        state_dir,
        control_url,
        &state,
        &mut last_applied_network_map_version,
    )
    .await?;
    run_agent_loop(
        &client,
        &backend,
        &dns_runtime,
        state_dir,
        control_url,
        &state,
        &mut last_applied_network_map_version,
    )
    .await?;

    Ok(state)
}

async fn register_agent_if_needed(
    client: &reqwest::Client,
    state_dir: &PathBuf,
    control_url: &str,
    login_key: &str,
    state: &mut AgentState,
) -> Result<()> {
    if state.device_id.is_some() {
        return Ok(());
    }

    let response: RegisterDeviceResponse = client
        .post(format!("{control_url}/v1/agent/register"))
        .json(&RegisterDeviceRequest {
            auth_key: login_key.to_string(),
            hostname: hostname(),
            machine_key: state.machine_key.clone(),
            backend_type: "tfscale".to_string(),
            backend_public_credential: state.backend_public_credential.clone(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    state.device_id = Some(response.device_id);
    state.node_key = Some(response.node_key);
    state.network_id = Some(response.network_id);
    state.ipv4 = Some(response.ipv4);
    state.poll_interval_seconds = response.poll_interval_seconds;
    state.save(state_dir)?;

    Ok(())
}

async fn ensure_backend_credentials(
    backend: &impl NetworkBackend,
    state: &mut AgentState,
) -> Result<()> {
    if state.backend_public_credential.is_empty() || state.device_id.is_none() {
        let backend_credential = backend.ensure_credentials().await?;
        state.backend_public_credential = backend_credential.public;
    }

    Ok(())
}

async fn run_agent_loop(
    client: &reqwest::Client,
    backend: &impl NetworkBackend,
    dns_runtime: &dns::DnsRuntime,
    state_dir: &PathBuf,
    control_url: &str,
    state: &AgentState,
    last_applied_network_map_version: &mut Option<i64>,
) -> Result<()> {
    let mut sync_interval =
        tokio::time::interval(Duration::from_secs(state.poll_interval_seconds.max(1)));
    let mut path_maintenance_interval = tokio::time::interval(DIRECT_PATH_MAINTENANCE_INTERVAL);

    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to listen for Ctrl+C")?;
                info!("agent shutdown requested");
                backend.shutdown().await?;
                break;
            }
            _ = sync_interval.tick() => {
                if let Err(error) = sync_agent_once(
                    client,
                    backend,
                    dns_runtime,
                    state_dir,
                    control_url,
                    state,
                    last_applied_network_map_version,
                )
                .await
                {
                    warn!(%error, "agent sync failed");
                }
            }
            _ = path_maintenance_interval.tick() => {
                if let Err(error) = backend.maintain_peer_paths().await {
                    warn!(%error, "peer path maintenance failed");
                }
            }
        }
    }

    Ok(())
}

async fn sync_agent_once(
    client: &reqwest::Client,
    backend: &impl NetworkBackend,
    dns_runtime: &dns::DnsRuntime,
    state_dir: &PathBuf,
    control_url: &str,
    state: &AgentState,
    last_applied_network_map_version: &mut Option<i64>,
) -> Result<()> {
    send_heartbeat(client, backend, control_url, state).await?;
    let network_map = fetch_network_map(client, control_url, state).await?;
    apply_network_map_and_maintain_paths(
        backend,
        dns_runtime,
        state_dir,
        state,
        network_map,
        last_applied_network_map_version,
    )
    .await
}

async fn apply_network_map_and_maintain_paths(
    backend: &impl NetworkBackend,
    dns_runtime: &dns::DnsRuntime,
    state_dir: &PathBuf,
    state: &AgentState,
    network_map: NetworkMapResponse,
    last_applied_network_map_version: &mut Option<i64>,
) -> Result<()> {
    apply_network_map_if_changed(
        backend,
        dns_runtime,
        state_dir,
        state,
        network_map,
        last_applied_network_map_version,
    )
    .await?;
    backend.maintain_peer_paths().await?;
    Ok(())
}

async fn send_heartbeat(
    client: &reqwest::Client,
    backend: &impl NetworkBackend,
    control_url: &str,
    state: &AgentState,
) -> Result<()> {
    let (device_id, node_key) = device_credentials(state)?;
    let status = backend.status().await?;
    let local_endpoints = backend
        .local_endpoints()
        .await?
        .into_iter()
        .map(endpoint_to_payload)
        .collect::<Vec<_>>();
    let endpoints = discovered_endpoints(
        client,
        backend,
        control_url,
        device_id,
        node_key,
        local_endpoints,
    )
    .await;

    client
        .post(format!("{control_url}/v1/agent/heartbeat"))
        .json(&HeartbeatRequest {
            device_id: device_id.to_string(),
            node_key: node_key.to_string(),
            endpoints,
            backend_status: BackendStatusPayload {
                backend_type: status.backend_type.as_str().to_string(),
                interface: status.interface_name,
                healthy: status.healthy,
            },
        })
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

async fn discovered_endpoints(
    client: &reqwest::Client,
    backend: &impl NetworkBackend,
    control_url: &str,
    device_id: &str,
    node_key: &str,
    mut endpoints: Vec<EndpointPayload>,
) -> Vec<EndpointPayload> {
    match probe_public_endpoint(client, backend, control_url, device_id, node_key).await {
        Ok(Some(endpoint)) => endpoints.push(endpoint),
        Ok(None) => {}
        Err(error) => warn!(%error, "endpoint probe failed"),
    }

    endpoints
}

async fn probe_public_endpoint(
    client: &reqwest::Client,
    backend: &impl NetworkBackend,
    control_url: &str,
    device_id: &str,
    node_key: &str,
) -> Result<Option<EndpointPayload>> {
    let response: EndpointProbeResponse = client
        .post(format!("{control_url}/v1/agent/endpoint-probe"))
        .json(&EndpointProbeRequest {
            device_id: device_id.to_string(),
            node_key: node_key.to_string(),
            protocol: "udp".to_string(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(probe_server) = udp_probe_server(&response)? {
        if let Some(probe) = backend
            .probe_public_endpoint(probe_server, Duration::from_secs(1))
            .await?
        {
            return Ok(Some(endpoint_from_backend_probe(probe.observed_endpoint)));
        }
    }

    Ok(endpoint_from_probe_response(response))
}

fn udp_probe_server(response: &EndpointProbeResponse) -> Result<Option<SocketAddr>> {
    let Some(address) = response.udp_probe_address.as_ref() else {
        return Ok(None);
    };
    let Some(port) = response.udp_probe_port else {
        return Ok(None);
    };

    Ok(Some(SocketAddr::new(address.parse()?, port)))
}

fn endpoint_from_backend_probe(endpoint: Endpoint) -> EndpointPayload {
    EndpointPayload {
        kind: endpoint_kind_to_payload(endpoint.kind).to_string(),
        address: endpoint.address.to_string(),
        port: endpoint.port,
        protocol: transport_protocol_to_payload(endpoint.protocol).to_string(),
        source: Some("stun".to_string()),
        priority: Some(50),
        expires_at: None,
    }
}

fn endpoint_from_probe_response(response: EndpointProbeResponse) -> Option<EndpointPayload> {
    if response.observed_port == 0 {
        return None;
    }

    Some(EndpointPayload {
        kind: "public".to_string(),
        address: response.observed_address,
        port: response.observed_port,
        protocol: response.protocol,
        source: Some("stun".to_string()),
        priority: Some(50),
        expires_at: None,
    })
}

async fn fetch_network_map(
    client: &reqwest::Client,
    control_url: &str,
    state: &AgentState,
) -> Result<NetworkMapResponse> {
    let (device_id, node_key) = device_credentials(state)?;

    Ok(client
        .get(format!("{control_url}/v1/agent/network-map"))
        .query(&[("device_id", device_id), ("node_key", node_key)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

fn device_credentials(state: &AgentState) -> Result<(&str, &str)> {
    let device_id = state
        .device_id
        .as_deref()
        .context("agent state is missing device ID")?;
    let node_key = state
        .node_key
        .as_deref()
        .context("agent state is missing node key")?;

    Ok((device_id, node_key))
}

async fn apply_network_map_if_changed(
    backend: &impl NetworkBackend,
    dns_runtime: &dns::DnsRuntime,
    state_dir: &PathBuf,
    state: &AgentState,
    network_map: NetworkMapResponse,
    last_applied_network_map_version: &mut Option<i64>,
) -> Result<()> {
    if *last_applied_network_map_version == Some(network_map.network_map_version) {
        return Ok(());
    }

    let version = network_map.network_map_version;
    apply_network_map_to_backend(backend, state, &network_map).await?;
    save_dns_snapshot(state_dir, state, network_map.dns_records.clone()).await?;
    dns_runtime.set_records(network_map.dns_records);
    *last_applied_network_map_version = Some(version);

    Ok(())
}

async fn apply_network_map_to_backend(
    backend: &impl NetworkBackend,
    state: &AgentState,
    network_map: &NetworkMapResponse,
) -> Result<()> {
    let overlay_ip = state
        .ipv4
        .as_deref()
        .context("agent state is missing assigned overlay IP")?
        .parse::<Ipv4Addr>()
        .context("agent state contains invalid overlay IP")?;
    let device_id = state
        .device_id
        .clone()
        .context("agent state is missing device ID")?;

    backend
        .apply_local_config(LocalBackendConfig {
            device_id,
            interface_name: DEFAULT_INTERFACE_NAME.to_string(),
            overlay_ip,
            listen_port: DEFAULT_LISTEN_PORT,
        })
        .await?;
    backend
        .apply_relay_map(network_map_to_relay_configs(network_map.relays.clone()))
        .await?;
    backend
        .apply_peer_map(network_map_to_peer_configs(network_map.peers.clone())?)
        .await?;

    Ok(())
}

async fn save_dns_snapshot(
    state_dir: &PathBuf,
    state: &AgentState,
    dns_records: Vec<DnsRecord>,
) -> Result<()> {
    let mut updated_state = state.clone();
    updated_state.dns_records = dns_records;
    updated_state.save(state_dir)
}

fn network_map_to_relay_configs(
    relays: Vec<tfscale_core::protocol::RelayMetadata>,
) -> Vec<RelayConfig> {
    relays
        .into_iter()
        .filter(|relay| relay.healthy)
        .map(|relay| RelayConfig {
            relay_id: relay.relay_id,
            url: relay.url,
            region: relay.region,
        })
        .collect()
}

fn network_map_to_peer_configs(peers: Vec<NetworkMapPeer>) -> Result<Vec<PeerConfig>> {
    peers.into_iter().map(peer_to_config).collect()
}

fn peer_to_config(peer: NetworkMapPeer) -> Result<PeerConfig> {
    Ok(PeerConfig {
        device_id: DeviceId::from(peer.device_id),
        hostname: peer.hostname,
        overlay_ip: peer
            .ipv4
            .parse::<Ipv4Addr>()
            .with_context(|| format!("invalid peer overlay IP: {}", peer.ipv4))?,
        public_credential: BackendCredential {
            public: peer.backend_public_credential,
        },
        endpoints: peer
            .endpoints
            .into_iter()
            .map(endpoint_to_config)
            .collect::<Result<Vec<_>>>()?,
        allowed_routes: peer.allowed_routes,
    })
}

fn endpoint_to_config(endpoint: EndpointPayload) -> Result<Endpoint> {
    Ok(Endpoint {
        kind: parse_endpoint_kind(&endpoint.kind)?,
        address: endpoint
            .address
            .parse::<IpAddr>()
            .with_context(|| format!("invalid endpoint address: {}", endpoint.address))?,
        port: endpoint.port,
        protocol: parse_transport_protocol(&endpoint.protocol)?,
    })
}

fn endpoint_to_payload(endpoint: Endpoint) -> EndpointPayload {
    EndpointPayload {
        kind: endpoint_kind_to_payload(endpoint.kind).to_string(),
        address: endpoint.address.to_string(),
        port: endpoint.port,
        protocol: transport_protocol_to_payload(endpoint.protocol).to_string(),
        source: Some("local".to_string()),
        priority: Some(100),
        expires_at: None,
    }
}

fn parse_endpoint_kind(value: &str) -> Result<EndpointKind> {
    match value {
        "lan" => Ok(EndpointKind::Lan),
        "public" => Ok(EndpointKind::Public),
        "ipv6" => Ok(EndpointKind::Ipv6),
        "relay" => Ok(EndpointKind::Relay),
        other => bail!("unsupported endpoint kind: {other}"),
    }
}

fn endpoint_kind_to_payload(value: EndpointKind) -> &'static str {
    match value {
        EndpointKind::Lan => "lan",
        EndpointKind::Public => "public",
        EndpointKind::Ipv6 => "ipv6",
        EndpointKind::Relay => "relay",
    }
}

fn parse_transport_protocol(value: &str) -> Result<TransportProtocol> {
    match value {
        "udp" => Ok(TransportProtocol::Udp),
        "tcp" => Ok(TransportProtocol::Tcp),
        other => bail!("unsupported endpoint protocol: {other}"),
    }
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tfscale")
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "tfscale-node".to_string())
}

fn default_dns_listen_string() -> String {
    DEFAULT_DNS_LISTEN.to_string()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AgentState {
    machine_key: String,
    backend_public_credential: String,
    device_id: Option<String>,
    node_key: Option<String>,
    network_id: Option<String>,
    ipv4: Option<String>,
    poll_interval_seconds: u64,
    #[serde(default)]
    dns_records: Vec<DnsRecord>,
    #[serde(default)]
    dns_enabled: bool,
    #[serde(default = "default_dns_listen_string")]
    dns_listen: String,
    #[serde(default)]
    dns_healthy: bool,
    #[serde(default)]
    dns_message: Option<String>,
}

impl AgentState {
    fn new() -> Self {
        Self {
            machine_key: format!("machine_{}", Uuid::now_v7().simple()),
            poll_interval_seconds: 5,
            dns_listen: DEFAULT_DNS_LISTEN.to_string(),
            ..Self::default()
        }
    }

    fn load(state_dir: &PathBuf) -> Result<Option<Self>> {
        let path = state_file(state_dir);
        if !path.exists() {
            return Ok(None);
        }

        let bytes = fs::read(path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    fn save(&self, state_dir: &PathBuf) -> Result<()> {
        fs::create_dir_all(state_dir)?;
        let bytes = serde_json::to_vec_pretty(self)?;
        fs::write(state_file(state_dir), bytes)?;
        Ok(())
    }
}

fn state_file(state_dir: &PathBuf) -> PathBuf {
    state_dir.join("state.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tfscale_core::protocol::NetworkMapSelf;
    use tfscale_net::{BackendStatus, BackendType, PeerPathDiagnostic, testing::MockBackend};

    #[tokio::test]
    async fn new_agent_state_gets_backend_credentials_from_backend() {
        let backend = MockBackend::new("mock-public-key");
        let mut state = AgentState::new();

        ensure_backend_credentials(&backend, &mut state)
            .await
            .expect("backend credentials");

        let snapshot = backend.snapshot();
        assert_eq!(snapshot.ensure_credentials_calls, 1);
        assert_eq!(state.backend_public_credential, "mock-public-key");
    }

    #[tokio::test]
    async fn unregistered_agent_state_refreshes_stale_backend_credentials() {
        let backend = MockBackend::new("mock-public-key");
        let mut state = AgentState {
            backend_public_credential: "tfpk_stale".to_string(),
            device_id: None,
            ..AgentState::new()
        };

        ensure_backend_credentials(&backend, &mut state)
            .await
            .expect("backend credentials");

        let snapshot = backend.snapshot();
        assert_eq!(snapshot.ensure_credentials_calls, 1);
        assert_eq!(state.backend_public_credential, "mock-public-key");
    }

    #[test]
    fn doctor_reports_unregistered_agent_failure() {
        let state = AgentState::new();
        let status = test_backend_status(Vec::new(), true);

        let report = AgentDoctorReport::from_state_and_status(&state, &status);

        assert_eq!(report.overall, DoctorLevel::Fail);
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "state.registered")
                .expect("registered check")
                .level,
            DoctorLevel::Fail
        );
    }

    #[test]
    fn doctor_reports_dns_snapshot_and_peer_summary() {
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            node_key: Some("node_self".to_string()),
            network_id: Some("net_default".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            dns_enabled: true,
            dns_healthy: true,
            dns_listen: "127.0.0.1:1053".to_string(),
            dns_records: vec![DnsRecord {
                device_id: "dev_peer".to_string(),
                name: "peer.mesh".to_string(),
                record_type: "A".to_string(),
                value: "100.64.0.3".to_string(),
            }],
            ..AgentState::new()
        };
        let status = test_backend_status(
            vec![
                test_peer_path("dev_a", PeerPathDiagnosticKind::Direct),
                test_peer_path("dev_b", PeerPathDiagnosticKind::Relay),
            ],
            true,
        );

        let report = AgentDoctorReport::from_state_and_status(&state, &status);

        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "dns.snapshot")
                .expect("dns snapshot check")
                .level,
            DoctorLevel::Ok
        );
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "backend.peers")
                .expect("peer check")
                .summary,
            "direct=1 relay=1 unknown=0"
        );
    }

    #[tokio::test]
    async fn applies_network_map_to_backend() {
        let backend = MockBackend::new("mock-public-key");
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            ..AgentState::new()
        };
        let network_map = NetworkMapResponse {
            network_map_version: 1,
            self_device: NetworkMapSelf {
                device_id: "dev_self".to_string(),
                hostname: "self".to_string(),
                ipv4: "100.64.0.2".to_string(),
                backend_type: "tfscale".to_string(),
            },
            peers: vec![NetworkMapPeer {
                device_id: "dev_peer".to_string(),
                hostname: "peer".to_string(),
                ipv4: "100.64.0.3".to_string(),
                backend_type: "tfscale".to_string(),
                backend_public_credential: "peer-public-key".to_string(),
                endpoints: vec![test_endpoint_payload("lan", "192.168.1.30", 51820, "udp")],
                allowed_routes: vec!["100.64.0.3/32".to_string()],
            }],
            dns_records: Vec::new(),
            relays: Vec::new(),
        };

        apply_network_map_to_backend(&backend, &state, &network_map)
            .await
            .expect("apply network map");

        let snapshot = backend.snapshot();
        assert_eq!(snapshot.local_configs.len(), 1);
        assert_eq!(
            snapshot.local_configs[0].overlay_ip,
            Ipv4Addr::new(100, 64, 0, 2)
        );
        assert_eq!(snapshot.peer_maps.len(), 1);
        assert_eq!(snapshot.peer_maps[0][0].device_id.as_str(), "dev_peer");
        assert_eq!(snapshot.peer_maps[0][0].hostname, "peer");
        assert_eq!(snapshot.relay_maps.len(), 1);
    }

    #[tokio::test]
    async fn applies_relay_map_to_backend() {
        let backend = MockBackend::new("mock-public-key");
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            ..AgentState::new()
        };
        let mut network_map = network_map_with_version(1);
        network_map.relays = vec![
            tfscale_core::protocol::RelayMetadata {
                relay_id: "relay_1".to_string(),
                url: "tcp://127.0.0.1:9443".to_string(),
                region: "local".to_string(),
                healthy: true,
            },
            tfscale_core::protocol::RelayMetadata {
                relay_id: "relay_down".to_string(),
                url: "tcp://127.0.0.1:9444".to_string(),
                region: "local".to_string(),
                healthy: false,
            },
        ];

        apply_network_map_to_backend(&backend, &state, &network_map)
            .await
            .expect("apply network map");

        let snapshot = backend.snapshot();
        assert_eq!(snapshot.relay_maps.len(), 1);
        assert_eq!(snapshot.relay_maps[0].len(), 1);
        assert_eq!(snapshot.relay_maps[0][0].relay_id, "relay_1");
        assert_eq!(snapshot.relay_maps[0][0].url, "tcp://127.0.0.1:9443");
    }

    #[tokio::test]
    async fn skips_unchanged_network_map_version() {
        let backend = MockBackend::new("mock-public-key");
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            ..AgentState::new()
        };
        let state_dir = temp_state_dir("skip-unchanged-network-map");
        let dns_runtime = test_dns_runtime();
        let mut last_applied_version = Some(7);

        apply_network_map_if_changed(
            &backend,
            &dns_runtime,
            &state_dir,
            &state,
            network_map_with_version(7),
            &mut last_applied_version,
        )
        .await
        .expect("skip unchanged network map");

        let snapshot = backend.snapshot();
        assert_eq!(snapshot.local_configs.len(), 0);
        assert_eq!(snapshot.peer_maps.len(), 0);
        assert_eq!(last_applied_version, Some(7));
    }

    #[tokio::test]
    async fn applies_changed_network_map_version() {
        let backend = MockBackend::new("mock-public-key");
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            ..AgentState::new()
        };
        let state_dir = temp_state_dir("apply-changed-network-map");
        let dns_runtime = test_dns_runtime();
        let mut last_applied_version = Some(7);

        apply_network_map_if_changed(
            &backend,
            &dns_runtime,
            &state_dir,
            &state,
            network_map_with_version(8),
            &mut last_applied_version,
        )
        .await
        .expect("apply changed network map");

        let snapshot = backend.snapshot();
        assert_eq!(snapshot.local_configs.len(), 1);
        assert_eq!(snapshot.peer_maps.len(), 1);
        assert_eq!(last_applied_version, Some(8));
    }

    #[tokio::test]
    async fn stores_dns_snapshot_from_network_map() {
        let backend = MockBackend::new("mock-public-key");
        let state_dir = temp_state_dir("dns-snapshot");
        let dns_runtime = test_dns_runtime();
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            ..AgentState::new()
        };
        state.save(&state_dir).expect("save initial state");
        let mut network_map = network_map_with_version(9);
        network_map.dns_records = vec![DnsRecord {
            device_id: "dev_peer".to_string(),
            name: "peer.mesh".to_string(),
            record_type: "A".to_string(),
            value: "100.64.0.3".to_string(),
        }];
        let mut last_applied_version = Some(8);

        apply_network_map_if_changed(
            &backend,
            &dns_runtime,
            &state_dir,
            &state,
            network_map,
            &mut last_applied_version,
        )
        .await
        .expect("apply DNS snapshot");

        let saved = AgentState::load(&state_dir)
            .expect("load state")
            .expect("state should exist");
        assert_eq!(saved.dns_records.len(), 1);
        assert_eq!(saved.dns_records[0].name, "peer.mesh");
        assert_eq!(saved.dns_records[0].value, "100.64.0.3");
        assert_eq!(dns_runtime.records_len(), 1);
    }

    #[tokio::test]
    async fn maintains_peer_paths_after_changed_network_map() {
        let backend = MockBackend::new("mock-public-key");
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            ..AgentState::new()
        };
        let state_dir = temp_state_dir("maintain-after-changed");
        let dns_runtime = test_dns_runtime();
        let mut last_applied_version = None;

        apply_network_map_and_maintain_paths(
            &backend,
            &dns_runtime,
            &state_dir,
            &state,
            network_map_with_version(8),
            &mut last_applied_version,
        )
        .await
        .expect("maintain paths");

        let snapshot = backend.snapshot();
        assert_eq!(snapshot.peer_maps.len(), 1);
        assert_eq!(snapshot.maintain_peer_paths_calls, 1);
    }

    #[tokio::test]
    async fn maintains_peer_paths_when_network_map_is_unchanged() {
        let backend = MockBackend::new("mock-public-key");
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            ..AgentState::new()
        };
        let state_dir = temp_state_dir("maintain-unchanged");
        let dns_runtime = test_dns_runtime();
        let mut last_applied_version = Some(8);

        apply_network_map_and_maintain_paths(
            &backend,
            &dns_runtime,
            &state_dir,
            &state,
            network_map_with_version(8),
            &mut last_applied_version,
        )
        .await
        .expect("maintain paths");

        let snapshot = backend.snapshot();
        assert_eq!(snapshot.peer_maps.len(), 0);
        assert_eq!(snapshot.maintain_peer_paths_calls, 1);
    }

    #[test]
    fn converts_network_map_peer_to_backend_config() {
        let peers = vec![NetworkMapPeer {
            device_id: "dev_test".to_string(),
            hostname: "devbox".to_string(),
            ipv4: "100.64.0.3".to_string(),
            backend_type: "tfscale".to_string(),
            backend_public_credential: "peer-public-key".to_string(),
            endpoints: vec![test_endpoint_payload("lan", "192.168.1.30", 51820, "udp")],
            allowed_routes: vec!["100.64.0.3/32".to_string()],
        }];

        let configs = network_map_to_peer_configs(peers).expect("peer config conversion");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].device_id.as_str(), "dev_test");
        assert_eq!(configs[0].hostname, "devbox");
        assert_eq!(configs[0].overlay_ip, Ipv4Addr::new(100, 64, 0, 3));
        assert_eq!(configs[0].public_credential.public, "peer-public-key");
        assert_eq!(configs[0].endpoints[0].kind, EndpointKind::Lan);
        assert_eq!(configs[0].endpoints[0].protocol, TransportProtocol::Udp);
        assert_eq!(configs[0].allowed_routes, vec!["100.64.0.3/32"]);
    }

    #[test]
    fn converts_backend_endpoint_to_heartbeat_payload() {
        let payload = endpoint_to_payload(Endpoint {
            kind: EndpointKind::Lan,
            address: IpAddr::from(Ipv4Addr::new(192, 168, 1, 30)),
            port: 51820,
            protocol: TransportProtocol::Udp,
        });

        assert_eq!(payload.kind, "lan");
        assert_eq!(payload.address, "192.168.1.30");
        assert_eq!(payload.port, 51820);
        assert_eq!(payload.protocol, "udp");
        assert_eq!(payload.source.as_deref(), Some("local"));
        assert_eq!(payload.priority, Some(100));
        assert_eq!(payload.expires_at, None);
    }

    #[test]
    fn converts_probe_response_to_public_endpoint() {
        let endpoint = endpoint_from_probe_response(EndpointProbeResponse {
            observed_address: "203.0.113.10".to_string(),
            observed_port: 49201,
            protocol: "udp".to_string(),
            udp_probe_address: None,
            udp_probe_port: None,
        })
        .expect("public endpoint");

        assert_eq!(endpoint.kind, "public");
        assert_eq!(endpoint.address, "203.0.113.10");
        assert_eq!(endpoint.port, 49201);
        assert_eq!(endpoint.protocol, "udp");
        assert_eq!(endpoint.source.as_deref(), Some("stun"));
        assert_eq!(endpoint.priority, Some(50));
    }

    #[test]
    fn skips_probe_response_without_port() {
        let endpoint = endpoint_from_probe_response(EndpointProbeResponse {
            observed_address: "203.0.113.10".to_string(),
            observed_port: 0,
            protocol: "udp".to_string(),
            udp_probe_address: None,
            udp_probe_port: None,
        });

        assert!(endpoint.is_none());
    }

    #[test]
    fn parses_udp_probe_server_from_probe_response() {
        let server = udp_probe_server(&EndpointProbeResponse {
            observed_address: "203.0.113.10".to_string(),
            observed_port: 49201,
            protocol: "udp".to_string(),
            udp_probe_address: Some("127.0.0.1".to_string()),
            udp_probe_port: Some(3478),
        })
        .expect("probe server")
        .expect("probe server");

        assert_eq!(server, "127.0.0.1:3478".parse().expect("socket addr"));
    }

    #[test]
    fn converts_backend_probe_to_public_endpoint_payload() {
        let endpoint = endpoint_from_backend_probe(Endpoint {
            kind: EndpointKind::Public,
            address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
            port: 49201,
            protocol: TransportProtocol::Udp,
        });

        assert_eq!(endpoint.kind, "public");
        assert_eq!(endpoint.address, "203.0.113.10");
        assert_eq!(endpoint.port, 49201);
        assert_eq!(endpoint.protocol, "udp");
        assert_eq!(endpoint.source.as_deref(), Some("stun"));
        assert_eq!(endpoint.priority, Some(50));
    }

    #[test]
    fn rejects_invalid_peer_overlay_ip() {
        let peer = NetworkMapPeer {
            device_id: "dev_test".to_string(),
            hostname: "devbox".to_string(),
            ipv4: "not-an-ip".to_string(),
            backend_type: "tfscale".to_string(),
            backend_public_credential: "peer-public-key".to_string(),
            endpoints: Vec::new(),
            allowed_routes: Vec::new(),
        };

        let error = peer_to_config(peer).expect_err("invalid IP should fail");

        assert!(error.to_string().contains("invalid peer overlay IP"));
    }

    #[test]
    fn rejects_unknown_endpoint_kind() {
        let endpoint = EndpointPayload {
            kind: "bluetooth".to_string(),
            address: "192.168.1.30".to_string(),
            port: 51820,
            protocol: "udp".to_string(),
            source: None,
            priority: None,
            expires_at: None,
        };

        let error = endpoint_to_config(endpoint).expect_err("unknown kind should fail");

        assert!(error.to_string().contains("unsupported endpoint kind"));
    }

    fn network_map_with_version(version: i64) -> NetworkMapResponse {
        NetworkMapResponse {
            network_map_version: version,
            self_device: NetworkMapSelf {
                device_id: "dev_self".to_string(),
                hostname: "self".to_string(),
                ipv4: "100.64.0.2".to_string(),
                backend_type: "tfscale".to_string(),
            },
            peers: vec![NetworkMapPeer {
                device_id: "dev_peer".to_string(),
                hostname: "peer".to_string(),
                ipv4: "100.64.0.3".to_string(),
                backend_type: "tfscale".to_string(),
                backend_public_credential: "peer-public-key".to_string(),
                endpoints: Vec::new(),
                allowed_routes: vec!["100.64.0.3/32".to_string()],
            }],
            dns_records: Vec::new(),
            relays: Vec::new(),
        }
    }

    fn temp_state_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "tfscale-agent-test-{name}-{}",
            Uuid::now_v7().simple()
        ));
        fs::create_dir_all(&path).expect("create temp state dir");
        path
    }

    fn test_dns_runtime() -> dns::DnsRuntime {
        dns::DnsRuntime::new(Vec::new())
    }

    fn test_backend_status(peers: Vec<PeerPathDiagnostic>, healthy: bool) -> BackendStatus {
        BackendStatus {
            backend_type: BackendType::Tfscale,
            interface_name: "tfscale0".to_string(),
            healthy,
            message: Some("test backend".to_string()),
            peers,
        }
    }

    fn test_peer_path(device_id: &str, path: PeerPathDiagnosticKind) -> PeerPathDiagnostic {
        PeerPathDiagnostic {
            device_id: device_id.to_string(),
            path,
            endpoint: None,
            rtt_ms: None,
            failures: 0,
            tx_packets: 0,
            rx_packets: 0,
        }
    }

    fn test_endpoint_payload(
        kind: &str,
        address: &str,
        port: u16,
        protocol: &str,
    ) -> EndpointPayload {
        EndpointPayload {
            kind: kind.to_string(),
            address: address.to_string(),
            port,
            protocol: protocol.to_string(),
            source: None,
            priority: None,
            expires_at: None,
        }
    }
}

fn transport_protocol_to_payload(value: TransportProtocol) -> &'static str {
    match value {
        TransportProtocol::Udp => "udp",
        TransportProtocol::Tcp => "tcp",
    }
}
