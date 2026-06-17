use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
};
use tfscale_core::DeviceId;
use tfscale_core::protocol::{
    BackendStatusPayload, EndpointPayload, HeartbeatRequest, NetworkMapPeer, NetworkMapResponse,
    RegisterDeviceRequest, RegisterDeviceResponse,
};
use tfscale_custom::CustomBackend;
use tfscale_net::{
    BackendCredential, Endpoint, EndpointKind, LocalBackendConfig, NetworkBackend, PeerConfig,
    TransportProtocol,
};
use tracing::info;
use uuid::Uuid;

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
    Status,
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
            let state = agent_up(&state_dir, &control_url, &login_key).await?;
            println!(
                "agent registered: device={} ipv4={} network={}",
                state.device_id.as_deref().unwrap_or("-"),
                state.ipv4.as_deref().unwrap_or("-"),
                state.network_id.as_deref().unwrap_or("-")
            );
        }
        Command::Down => {
            let backend = CustomBackend::default();
            backend.shutdown().await?;
            println!("agent backend stopped");
        }
        Command::Status => {
            let backend = CustomBackend::default();
            let status = backend.status().await?;
            let state = AgentState::load(&state_dir)?.unwrap_or_default();
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

    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

async fn agent_up(state_dir: &PathBuf, control_url: &str, login_key: &str) -> Result<AgentState> {
    let client = reqwest::Client::new();
    let mut state = AgentState::load(state_dir)?.unwrap_or_else(AgentState::new);
    let backend = CustomBackend::default();

    if state.backend_public_credential.is_empty() {
        let backend_credential = backend.ensure_credentials().await?;
        state.backend_public_credential = backend_credential.public;
    }

    if state.device_id.is_none() {
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
    }

    if let (Some(device_id), Some(node_key)) = (&state.device_id, &state.node_key) {
        client
            .post(format!("{control_url}/v1/agent/heartbeat"))
            .json(&HeartbeatRequest {
                device_id: device_id.clone(),
                node_key: node_key.clone(),
                endpoints: Vec::new(),
                backend_status: BackendStatusPayload {
                    backend_type: "tfscale".to_string(),
                    interface: "tfscale0".to_string(),
                    healthy: false,
                },
            })
            .send()
            .await?
            .error_for_status()?;

        let network_map: NetworkMapResponse = client
            .get(format!("{control_url}/v1/agent/network-map"))
            .query(&[("device_id", device_id), ("node_key", node_key)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let overlay_ip = state
            .ipv4
            .as_deref()
            .context("agent state is missing assigned overlay IP")?
            .parse::<Ipv4Addr>()
            .context("agent state contains invalid overlay IP")?;

        backend
            .apply_local_config(LocalBackendConfig {
                interface_name: "tfscale0".to_string(),
                overlay_ip,
                listen_port: 51820,
            })
            .await?;
        backend
            .apply_peer_map(network_map_to_peer_configs(network_map.peers)?)
            .await?;
    }

    Ok(state)
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

fn parse_endpoint_kind(value: &str) -> Result<EndpointKind> {
    match value {
        "lan" => Ok(EndpointKind::Lan),
        "public" => Ok(EndpointKind::Public),
        "ipv6" => Ok(EndpointKind::Ipv6),
        "relay" => Ok(EndpointKind::Relay),
        other => bail!("unsupported endpoint kind: {other}"),
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
            backend_public_credential: format!("tfpk_{}", Uuid::now_v7().simple()),
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

    #[test]
    fn converts_network_map_peer_to_backend_config() {
        let peers = vec![NetworkMapPeer {
            device_id: "dev_test".to_string(),
            hostname: "devbox".to_string(),
            ipv4: "100.64.0.3".to_string(),
            backend_type: "tfscale".to_string(),
            backend_public_credential: "peer-public-key".to_string(),
            endpoints: vec![EndpointPayload {
                kind: "lan".to_string(),
                address: "192.168.1.30".to_string(),
                port: 51820,
                protocol: "udp".to_string(),
            }],
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
        };

        let error = endpoint_to_config(endpoint).expect_err("unknown kind should fail");

        assert!(error.to_string().contains("unsupported endpoint kind"));
    }
}
