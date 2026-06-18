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
    BackendStatusPayload, EndpointPayload, EndpointProbeRequest, EndpointProbeResponse,
    HeartbeatRequest, NetworkMapPeer, NetworkMapResponse, RegisterDeviceRequest,
    RegisterDeviceResponse,
};
use tfscale_custom::CustomBackend;
use tfscale_net::{
    BackendCredential, Endpoint, EndpointKind, LocalBackendConfig, NetworkBackend, PeerConfig,
    RelayConfig, TransportProtocol,
};
use tracing::{info, warn};
use uuid::Uuid;

const DEFAULT_INTERFACE_NAME: &str = "tfscale0";
const DEFAULT_LISTEN_PORT: u16 = 51820;

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
    },
    Down,
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
        } => {
            info!(%control_url, "agent up requested");
            agent_up(&state_dir, &control_url, &login_key).await?;
        }
        Command::Down => {
            let backend = CustomBackend::with_state_dir(DEFAULT_INTERFACE_NAME, &state_dir);
            backend.shutdown().await?;
            println!("agent backend stopped");
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
    backend_public_credential_present: bool,
    backend: BackendStatusOutput,
}

#[derive(Debug, Serialize)]
struct BackendStatusOutput {
    backend_type: String,
    interface_name: String,
    healthy: bool,
    message: Option<String>,
}

impl AgentStatusOutput {
    fn from_state_and_status(state: AgentState, status: tfscale_net::BackendStatus) -> Self {
        Self {
            device_id: state.device_id,
            network_id: state.network_id,
            ipv4: state.ipv4,
            backend_public_credential_present: !state.backend_public_credential.is_empty(),
            backend: BackendStatusOutput {
                backend_type: status.backend_type.as_str().to_string(),
                interface_name: status.interface_name,
                healthy: status.healthy,
                message: status.message,
            },
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

async fn agent_up(state_dir: &PathBuf, control_url: &str, login_key: &str) -> Result<AgentState> {
    let client = reqwest::Client::new();
    let mut state = AgentState::load(state_dir)?.unwrap_or_else(AgentState::new);
    let backend = CustomBackend::with_state_dir(DEFAULT_INTERFACE_NAME, state_dir);

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
        control_url,
        &state,
        &mut last_applied_network_map_version,
    )
    .await?;
    run_agent_loop(
        &client,
        &backend,
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
    control_url: &str,
    state: &AgentState,
    last_applied_network_map_version: &mut Option<i64>,
) -> Result<()> {
    let interval = Duration::from_secs(state.poll_interval_seconds.max(1));

    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to listen for Ctrl+C")?;
                info!("agent shutdown requested");
                backend.shutdown().await?;
                break;
            }
            _ = tokio::time::sleep(interval) => {
                if let Err(error) = sync_agent_once(
                    client,
                    backend,
                    control_url,
                    state,
                    last_applied_network_map_version,
                )
                .await
                {
                    warn!(%error, "agent sync failed");
                }
            }
        }
    }

    Ok(())
}

async fn sync_agent_once(
    client: &reqwest::Client,
    backend: &impl NetworkBackend,
    control_url: &str,
    state: &AgentState,
    last_applied_network_map_version: &mut Option<i64>,
) -> Result<()> {
    send_heartbeat(client, backend, control_url, state).await?;
    let network_map = fetch_network_map(client, control_url, state).await?;
    apply_network_map_and_maintain_paths(
        backend,
        state,
        network_map,
        last_applied_network_map_version,
    )
    .await
}

async fn apply_network_map_and_maintain_paths(
    backend: &impl NetworkBackend,
    state: &AgentState,
    network_map: NetworkMapResponse,
    last_applied_network_map_version: &mut Option<i64>,
) -> Result<()> {
    apply_network_map_if_changed(
        backend,
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
    state: &AgentState,
    network_map: NetworkMapResponse,
    last_applied_network_map_version: &mut Option<i64>,
) -> Result<()> {
    if *last_applied_network_map_version == Some(network_map.network_map_version) {
        return Ok(());
    }

    let version = network_map.network_map_version;
    apply_network_map_to_backend(backend, state, network_map).await?;
    *last_applied_network_map_version = Some(version);

    Ok(())
}

async fn apply_network_map_to_backend(
    backend: &impl NetworkBackend,
    state: &AgentState,
    network_map: NetworkMapResponse,
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
        .apply_relay_map(network_map_to_relay_configs(network_map.relays))
        .await?;
    backend
        .apply_peer_map(network_map_to_peer_configs(network_map.peers)?)
        .await?;

    Ok(())
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AgentState {
    machine_key: String,
    backend_public_credential: String,
    device_id: Option<String>,
    node_key: Option<String>,
    network_id: Option<String>,
    ipv4: Option<String>,
    poll_interval_seconds: u64,
}

impl AgentState {
    fn new() -> Self {
        Self {
            machine_key: format!("machine_{}", Uuid::now_v7().simple()),
            poll_interval_seconds: 5,
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
    use tfscale_net::testing::MockBackend;

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
            relays: Vec::new(),
        };

        apply_network_map_to_backend(&backend, &state, network_map)
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

        apply_network_map_to_backend(&backend, &state, network_map)
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
        let mut last_applied_version = Some(7);

        apply_network_map_if_changed(
            &backend,
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
        let mut last_applied_version = Some(7);

        apply_network_map_if_changed(
            &backend,
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
    async fn maintains_peer_paths_after_changed_network_map() {
        let backend = MockBackend::new("mock-public-key");
        let state = AgentState {
            device_id: Some("dev_self".to_string()),
            ipv4: Some("100.64.0.2".to_string()),
            ..AgentState::new()
        };
        let mut last_applied_version = None;

        apply_network_map_and_maintain_paths(
            &backend,
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
        let mut last_applied_version = Some(8);

        apply_network_map_and_maintain_paths(
            &backend,
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
            relays: Vec::new(),
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
