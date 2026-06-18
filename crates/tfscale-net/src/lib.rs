use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
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
    pub device_id: String,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicEndpointProbe {
    pub observed_endpoint: Endpoint,
}

#[async_trait]
pub trait NetworkBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    fn capabilities(&self) -> BackendCapabilities;

    async fn ensure_credentials(&self) -> Result<BackendCredential>;
    async fn apply_local_config(&self, config: LocalBackendConfig) -> Result<()>;
    async fn apply_peer_map(&self, peers: Vec<PeerConfig>) -> Result<()>;
    async fn local_endpoints(&self) -> Result<Vec<Endpoint>>;
    async fn probe_public_endpoint(
        &self,
        probe_server: SocketAddr,
        timeout: Duration,
    ) -> Result<Option<PublicEndpointProbe>>;
    async fn status(&self) -> Result<BackendStatus>;
    async fn shutdown(&self) -> Result<()>;
}

#[cfg(any(test, feature = "test-utils"))]
pub mod testing {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    pub struct MockBackend {
        state: Arc<Mutex<MockBackendState>>,
        credential: BackendCredential,
        backend_type: BackendType,
        capabilities: BackendCapabilities,
        endpoints: Vec<Endpoint>,
    }

    #[derive(Clone, Debug, Default)]
    pub struct MockBackendState {
        pub ensure_credentials_calls: usize,
        pub local_configs: Vec<LocalBackendConfig>,
        pub peer_maps: Vec<Vec<PeerConfig>>,
        pub shutdown_calls: usize,
    }

    impl MockBackend {
        pub fn new(public_credential: impl Into<String>) -> Self {
            Self {
                state: Arc::new(Mutex::new(MockBackendState::default())),
                credential: BackendCredential {
                    public: public_credential.into(),
                },
                backend_type: BackendType::Custom("mock".to_string()),
                capabilities: BackendCapabilities {
                    supports_userspace_tun: true,
                    supports_dynamic_peers: true,
                    ..BackendCapabilities::default()
                },
                endpoints: Vec::new(),
            }
        }

        pub fn with_endpoints(mut self, endpoints: Vec<Endpoint>) -> Self {
            self.endpoints = endpoints;
            self
        }

        pub fn snapshot(&self) -> MockBackendState {
            self.state.lock().expect("mock backend state lock").clone()
        }
    }

    #[async_trait]
    impl NetworkBackend for MockBackend {
        fn backend_type(&self) -> BackendType {
            self.backend_type.clone()
        }

        fn capabilities(&self) -> BackendCapabilities {
            self.capabilities.clone()
        }

        async fn ensure_credentials(&self) -> Result<BackendCredential> {
            self.state
                .lock()
                .expect("mock backend state lock")
                .ensure_credentials_calls += 1;
            Ok(self.credential.clone())
        }

        async fn apply_local_config(&self, config: LocalBackendConfig) -> Result<()> {
            self.state
                .lock()
                .expect("mock backend state lock")
                .local_configs
                .push(config);
            Ok(())
        }

        async fn apply_peer_map(&self, peers: Vec<PeerConfig>) -> Result<()> {
            self.state
                .lock()
                .expect("mock backend state lock")
                .peer_maps
                .push(peers);
            Ok(())
        }

        async fn local_endpoints(&self) -> Result<Vec<Endpoint>> {
            Ok(self.endpoints.clone())
        }

        async fn probe_public_endpoint(
            &self,
            _probe_server: SocketAddr,
            _timeout: Duration,
        ) -> Result<Option<PublicEndpointProbe>> {
            Ok(None)
        }

        async fn status(&self) -> Result<BackendStatus> {
            Ok(BackendStatus {
                backend_type: self.backend_type(),
                interface_name: "mock0".to_string(),
                healthy: true,
                message: None,
            })
        }

        async fn shutdown(&self) -> Result<()> {
            self.state
                .lock()
                .expect("mock backend state lock")
                .shutdown_calls += 1;
            Ok(())
        }
    }
}
