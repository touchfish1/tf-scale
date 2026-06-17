use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use tfscale_core::DeviceId;

pub type Result<T> = std::result::Result<T, BackendError>;

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("required command is missing: {0}")]
    MissingCommand(String),

    #[error("backend command failed: {0}")]
    CommandFailed(String),

    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendType {
    Tfscale,
    WireGuard,
    EasyTier,
    Custom(String),
}

impl BackendType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Tfscale => "tfscale",
            Self::WireGuard => "wireguard",
            Self::EasyTier => "easytier",
            Self::Custom(value) => value.as_str(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub supports_relay: bool,
    pub supports_nat_traversal: bool,
    pub supports_kernel_tun: bool,
    pub supports_userspace_tun: bool,
    pub supports_dynamic_peers: bool,
    pub supports_static_peers: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackendCredential {
    pub public: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalBackendConfig {
    pub interface_name: String,
    pub overlay_ip: Ipv4Addr,
    pub listen_port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub kind: EndpointKind,
    pub address: IpAddr,
    pub port: u16,
    pub protocol: TransportProtocol,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    Lan,
    Public,
    Ipv6,
    Relay,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportProtocol {
    Udp,
    Tcp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PeerConfig {
    pub device_id: DeviceId,
    pub hostname: String,
    pub overlay_ip: Ipv4Addr,
    pub public_credential: BackendCredential,
    pub endpoints: Vec<Endpoint>,
    pub allowed_routes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackendStatus {
    pub backend_type: BackendType,
    pub interface_name: String,
    pub healthy: bool,
    pub message: Option<String>,
}

#[async_trait]
pub trait NetworkBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    fn capabilities(&self) -> BackendCapabilities;

    async fn ensure_credentials(&self) -> Result<BackendCredential>;
    async fn apply_local_config(&self, config: LocalBackendConfig) -> Result<()>;
    async fn apply_peer_map(&self, peers: Vec<PeerConfig>) -> Result<()>;
    async fn status(&self) -> Result<BackendStatus>;
    async fn shutdown(&self) -> Result<()>;
}
