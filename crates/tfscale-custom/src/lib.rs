use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tfscale_net::{
    BackendCapabilities, BackendCredential, BackendError, BackendStatus, BackendType, Endpoint,
    LocalBackendConfig, NetworkBackend, PeerConfig, Result,
};
use uuid::Uuid;
use x25519_dalek::{PublicKey, StaticSecret};

mod crypto;
mod frame;
mod nonce;
mod packet;
mod platform;
mod transport;
mod tun;

use crypto::decode_public_credential;
use transport::{TransportRuntime, select_udp_endpoint};
use tun::{TunConfig, TunDevice, TunStatus};

const STATE_VERSION: u32 = 1;
const IDENTITY_SCHEME: &str = "x25519";
const PUBLIC_CREDENTIAL_PREFIX: &str = "tfpk1_";

#[derive(Clone)]
pub struct CustomBackend {
    interface_name: String,
    state_path: PathBuf,
    state: Arc<Mutex<RuntimeState>>,
    tun_setup_enabled: bool,
}

impl CustomBackend {
    pub fn new(interface_name: impl Into<String>) -> Self {
        Self::with_state_path(interface_name, default_state_path())
    }

    pub fn with_state_dir(interface_name: impl Into<String>, state_dir: impl AsRef<Path>) -> Self {
        Self::with_state_path(
            interface_name,
            state_dir.as_ref().join("custom-backend.json"),
        )
    }

    pub fn with_state_path(
        interface_name: impl Into<String>,
        state_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            interface_name: interface_name.into(),
            state_path: state_path.into(),
            state: Arc::new(Mutex::new(RuntimeState::default())),
            tun_setup_enabled: true,
        }
    }

    pub fn with_tun_setup(mut self, enabled: bool) -> Self {
        self.tun_setup_enabled = enabled;
        self
    }

    fn load_state(&self) -> Result<CustomBackendState> {
        if !self.state_path.exists() {
            return Ok(CustomBackendState::default());
        }

        let bytes = fs::read(&self.state_path)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        let state: CustomBackendState = serde_json::from_slice(&bytes)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        validate_state_version(&state)?;
        Ok(state)
    }

    fn save_state(&self, state: &CustomBackendState) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        }

        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        let temporary_path = self.state_path.with_extension("json.tmp");
        fs::write(&temporary_path, bytes)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        fs::rename(&temporary_path, &self.state_path)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        protect_state_file(&self.state_path)?;
        Ok(())
    }

    fn replace_state(&self, state: CustomBackendState) -> Result<()> {
        self.save_state(&state)?;
        let mut runtime = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        let tun_status = runtime.tun_status.clone();
        let tun_device = runtime.tun_device.take();
        let transport = runtime.transport.take();
        *runtime = RuntimeState::from_persisted(state);
        runtime.tun_status = tun_status;
        runtime.tun_device = tun_device;
        runtime.transport = transport;
        Ok(())
    }

    fn set_tun_status(&self, status: TunStatus) -> Result<()> {
        self.state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .tun_status = Some(status);
        Ok(())
    }

    fn set_tun_device(&self, device: TunDevice) -> Result<()> {
        let status = device.status();
        let mut runtime = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        runtime.tun_status = Some(status);
        runtime.tun_device = Some(device);
        Ok(())
    }

    fn bind_transport(&self, listen_port: u16) -> Result<()> {
        self.state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport
            .take();
        let transport = TransportRuntime::bind(listen_port)?;
        self.state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport = Some(transport);
        Ok(())
    }
}

impl Default for CustomBackend {
    fn default() -> Self {
        Self::new("tfscale0")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CustomBackendState {
    version: u32,
    identity: Option<CustomIdentity>,
    local_config: Option<LocalBackendConfig>,
    peers: Vec<StoredPeerSession>,
}

impl Default for CustomBackendState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            identity: None,
            local_config: None,
            peers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CustomIdentity {
    key_id: String,
    scheme: String,
    private_key: String,
    public_key: String,
    created_at_unix_seconds: u64,
    rotated_from: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredPeerSession {
    device_id: String,
    hostname: String,
    overlay_ip: Ipv4Addr,
    public_key: String,
    endpoints: Vec<Endpoint>,
    allowed_routes: Vec<String>,
    last_updated_at_unix_seconds: u64,
}

#[derive(Default)]
struct RuntimeState {
    persisted: CustomBackendState,
    peers_by_device: HashMap<String, StoredPeerSession>,
    peers_by_overlay_ip: HashMap<Ipv4Addr, String>,
    peer_crypto_material_by_device: HashMap<String, PeerCryptoMaterial>,
    peer_udp_endpoints_by_device: HashMap<String, Endpoint>,
    tun_status: Option<TunStatus>,
    tun_device: Option<TunDevice>,
    transport: Option<TransportRuntime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PeerCryptoMaterial {
    #[allow(dead_code)]
    public_key: [u8; 32],
}

impl RuntimeState {
    fn from_persisted(persisted: CustomBackendState) -> Self {
        let peers_by_device = persisted
            .peers
            .iter()
            .map(|peer| (peer.device_id.clone(), peer.clone()))
            .collect();
        let peers_by_overlay_ip = persisted
            .peers
            .iter()
            .map(|peer| (peer.overlay_ip, peer.device_id.clone()))
            .collect();
        let peer_crypto_material_by_device = persisted
            .peers
            .iter()
            .filter_map(|peer| {
                decode_public_credential(&peer.public_key)
                    .ok()
                    .map(|public_key| (peer.device_id.clone(), PeerCryptoMaterial { public_key }))
            })
            .collect();
        let peer_udp_endpoints_by_device = persisted
            .peers
            .iter()
            .filter_map(|peer| {
                select_udp_endpoint(&peer.endpoints)
                    .map(|endpoint| (peer.device_id.clone(), endpoint))
            })
            .collect();

        Self {
            persisted,
            peers_by_device,
            peers_by_overlay_ip,
            peer_crypto_material_by_device,
            peer_udp_endpoints_by_device,
            tun_status: None,
            tun_device: None,
            transport: None,
        }
    }
}

fn default_state_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tfscale")
        .join("custom-backend.json")
}

fn validate_state_version(state: &CustomBackendState) -> Result<()> {
    if state.version != STATE_VERSION {
        return Err(BackendError::CommandFailed(format!(
            "unsupported custom backend state version: {}",
            state.version
        )));
    }

    Ok(())
}

fn generate_identity() -> CustomIdentity {
    let private = StaticSecret::random();
    let public = PublicKey::from(&private);

    CustomIdentity {
        key_id: format!("kid_{}", Uuid::now_v7().simple()),
        scheme: IDENTITY_SCHEME.to_string(),
        private_key: encode_key(private.to_bytes()),
        public_key: encode_public_credential(public.to_bytes()),
        created_at_unix_seconds: now_unix_seconds(),
        rotated_from: None,
    }
}

fn derive_public_credential(private_key: &str) -> Result<String> {
    let private = StaticSecret::from(decode_key(private_key)?);
    let public = PublicKey::from(&private);
    Ok(encode_public_credential(public.to_bytes()))
}

fn validate_public_credential(value: &str) -> Result<()> {
    let encoded = value
        .strip_prefix(PUBLIC_CREDENTIAL_PREFIX)
        .ok_or_else(|| {
            BackendError::CommandFailed("unsupported peer credential prefix".to_string())
        })?;
    let bytes = decode_base64_key(encoded)?;
    if bytes.len() != 32 {
        return Err(BackendError::CommandFailed(format!(
            "peer credential has invalid key length: {}",
            bytes.len()
        )));
    }

    Ok(())
}

fn encode_public_credential(bytes: [u8; 32]) -> String {
    format!("{PUBLIC_CREDENTIAL_PREFIX}{}", encode_key(bytes))
}

fn encode_key(bytes: [u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_key(value: &str) -> Result<[u8; 32]> {
    let bytes = decode_base64_key(value)?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        BackendError::CommandFailed(format!("expected 32-byte key, got {}", bytes.len()))
    })
}

fn decode_base64_key(value: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(value.as_bytes())
        .map_err(|error| BackendError::CommandFailed(error.to_string()))
}

fn peer_to_session(peer: PeerConfig) -> Result<StoredPeerSession> {
    validate_public_credential(&peer.public_credential.public)?;

    Ok(StoredPeerSession {
        device_id: peer.device_id.to_string(),
        hostname: peer.hostname,
        overlay_ip: peer.overlay_ip,
        public_key: peer.public_credential.public,
        endpoints: peer.endpoints,
        allowed_routes: peer.allowed_routes,
        last_updated_at_unix_seconds: now_unix_seconds(),
    })
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn protect_state_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
}

#[async_trait]
impl NetworkBackend for CustomBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Custom("tfscale".to_string())
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            supports_userspace_tun: true,
            supports_static_peers: true,
            ..BackendCapabilities::default()
        }
    }

    async fn ensure_credentials(&self) -> Result<BackendCredential> {
        let mut state = self.load_state()?;
        if let Some(identity) = state.identity.clone() {
            let derived_public_key = derive_public_credential(&identity.private_key)?;
            if identity.public_key != derived_public_key {
                return Err(BackendError::CommandFailed(
                    "stored public key does not match private key".to_string(),
                ));
            }
            *self
                .state
                .lock()
                .map_err(|error| BackendError::CommandFailed(error.to_string()))? =
                RuntimeState::from_persisted(state);
            return Ok(BackendCredential {
                public: identity.public_key,
            });
        }

        let identity = generate_identity();
        let public = identity.public_key.clone();
        state.identity = Some(identity);
        self.replace_state(state)?;

        Ok(BackendCredential { public })
    }

    async fn apply_local_config(&self, config: LocalBackendConfig) -> Result<()> {
        let mut state = self.load_state()?;
        state.local_config = Some(config);
        self.replace_state(state)?;

        let config = self
            .load_state()?
            .local_config
            .expect("local config was just persisted");
        let tun_config = TunConfig::from_local_config(&config);
        self.bind_transport(tun_config.listen_port)?;
        if !self.tun_setup_enabled {
            let status = TunStatus::failed(
                tun_config.interface_name.clone(),
                "TUN setup skipped by backend configuration",
            );
            self.set_tun_status(status)?;
            return Ok(());
        }

        match TunDevice::configure(&tun_config) {
            Ok(device) => {
                self.set_tun_device(device)?;
                Ok(())
            }
            Err(error) => {
                self.set_tun_status(TunStatus::failed(
                    tun_config.interface_name,
                    error.to_string(),
                ))?;
                Err(error)
            }
        }
    }

    async fn apply_peer_map(&self, peers: Vec<PeerConfig>) -> Result<()> {
        let mut state = self.load_state()?;
        state.peers = peers
            .into_iter()
            .map(peer_to_session)
            .collect::<Result<Vec<_>>>()?;
        self.replace_state(state)?;
        Ok(())
    }

    async fn local_endpoints(&self) -> Result<Vec<Endpoint>> {
        Ok(self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport
            .as_ref()
            .map(TransportRuntime::local_endpoints)
            .unwrap_or_default())
    }

    async fn status(&self) -> Result<BackendStatus> {
        let state = self.load_state()?;
        let runtime = RuntimeState::from_persisted(state);
        let has_credentials = runtime.persisted.identity.is_some();
        let has_local_config = runtime.persisted.local_config.is_some();
        let tun_status = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .tun_status
            .clone();
        let tun_configured = tun_status
            .as_ref()
            .map(|status| status.configured)
            .unwrap_or(false);
        let tun_io_ready = tun_status
            .as_ref()
            .map(|status| status.io_ready)
            .unwrap_or(false);
        let tun_message = tun_status
            .and_then(|status| status.message)
            .unwrap_or_else(|| "not_configured".to_string());
        let transport_status = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport
            .as_ref()
            .map(TransportRuntime::status)
            .unwrap_or_default();
        let transport_peers = runtime.peer_udp_endpoints_by_device.len();
        let reachable_peers = transport_peers;
        Ok(BackendStatus {
            backend_type: self.backend_type(),
            interface_name: self.interface_name.clone(),
            healthy: tun_configured && tun_io_ready && transport_status.udp_bound,
            message: Some(format!(
                "custom backend state: credentials={} local_config={} peers={} peer_indexes=device:{} overlay:{} crypto_peers={} udp_bound={} local_endpoints={} transport_peers={} reachable_peers={} tx_packets={} rx_packets={} tx_drops={} rx_drops={} tun_configured={} tun_io_ready={} tun_message={} data_plane=transport_ready",
                has_credentials,
                has_local_config,
                runtime.persisted.peers.len(),
                runtime.peers_by_device.len(),
                runtime.peers_by_overlay_ip.len(),
                runtime.peer_crypto_material_by_device.len(),
                transport_status.udp_bound,
                transport_status.local_endpoints,
                transport_peers,
                reachable_peers,
                transport_status.tx_packets,
                transport_status.rx_packets,
                transport_status.tx_drops,
                transport_status.rx_drops,
                tun_configured,
                tun_io_ready,
                tun_message
            )),
        })
    }

    async fn shutdown(&self) -> Result<()> {
        self.state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport
            .take();
        let device = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .tun_device
            .take();

        if let Some(device) = device {
            let interface_name = device.status().interface_name;
            device.shutdown()?;
            self.set_tun_status(TunStatus::failed(interface_name, "TUN device shut down"))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use tfscale_core::DeviceId;
    use tfscale_net::{Endpoint, EndpointKind, TransportProtocol};

    #[test]
    fn defaults_to_tfscale_interface_name() {
        let backend = CustomBackend::default();

        assert_eq!(backend.interface_name, "tfscale0");
    }

    #[tokio::test]
    async fn persists_backend_credentials() {
        let state_path = temp_state_path("credentials");
        let backend = test_backend(&state_path);

        let first = backend
            .ensure_credentials()
            .await
            .expect("first credentials");
        let second = test_backend(&state_path)
            .ensure_credentials()
            .await
            .expect("second credentials");
        let state = read_state(&state_path);

        assert_eq!(first, second);
        let identity = state.identity.expect("identity");
        assert_eq!(identity.scheme, IDENTITY_SCHEME);
        assert_eq!(identity.public_key, first.public);
        assert_eq!(
            derive_public_credential(&identity.private_key).expect("derived public key"),
            first.public
        );
        assert!(!first.public.contains(&identity.private_key));

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn stores_local_config_and_peer_map() {
        let state_path = temp_state_path("peer-map");
        let backend = test_backend(&state_path);

        backend
            .ensure_credentials()
            .await
            .expect("backend credentials");
        backend
            .apply_local_config(LocalBackendConfig {
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");
        backend
            .apply_peer_map(vec![PeerConfig {
                device_id: DeviceId::from("dev_peer".to_string()),
                hostname: "peer".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 3),
                public_credential: BackendCredential {
                    public: test_public_credential(),
                },
                endpoints: vec![Endpoint {
                    kind: EndpointKind::Lan,
                    address: IpAddr::from(Ipv4Addr::new(192, 168, 1, 30)),
                    port: 51820,
                    protocol: TransportProtocol::Udp,
                }],
                allowed_routes: vec!["100.64.0.3/32".to_string()],
            }])
            .await
            .expect("peer map");

        let state = read_state(&state_path);
        assert_eq!(
            state.local_config.expect("local config").overlay_ip,
            Ipv4Addr::new(100, 64, 0, 2)
        );
        assert_eq!(state.peers.len(), 1);
        assert_eq!(state.peers[0].device_id.as_str(), "dev_peer");

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn rejects_unsupported_state_version() {
        let state_path = temp_state_path("unsupported-version");
        let state = CustomBackendState {
            version: STATE_VERSION + 1,
            ..CustomBackendState::default()
        };
        fs::write(&state_path, serde_json::to_vec(&state).expect("state json"))
            .expect("write state");

        let error = test_backend(&state_path)
            .ensure_credentials()
            .await
            .expect_err("unsupported version should fail");

        assert!(
            error
                .to_string()
                .contains("unsupported custom backend state version")
        );
        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn peer_map_replaces_stale_peers() {
        let state_path = temp_state_path("replace-peers");
        let backend = test_backend(&state_path);

        backend
            .apply_peer_map(vec![
                test_peer("dev_one", Ipv4Addr::new(100, 64, 0, 3)),
                test_peer("dev_two", Ipv4Addr::new(100, 64, 0, 4)),
            ])
            .await
            .expect("first peer map");
        backend
            .apply_peer_map(vec![test_peer("dev_two", Ipv4Addr::new(100, 64, 0, 4))])
            .await
            .expect("second peer map");

        let state = read_state(&state_path);
        assert_eq!(state.peers.len(), 1);
        assert_eq!(state.peers[0].device_id, "dev_two");

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn rejects_unsupported_peer_credential_format() {
        let state_path = temp_state_path("bad-peer-credential");
        let backend = test_backend(&state_path);
        let mut peer = test_peer("dev_bad", Ipv4Addr::new(100, 64, 0, 3));
        peer.public_credential.public = "peer-public-key".to_string();

        let error = backend
            .apply_peer_map(vec![peer])
            .await
            .expect_err("bad credential should fail");

        assert!(
            error
                .to_string()
                .contains("unsupported peer credential prefix")
        );
        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn status_reports_skipped_tun_setup() {
        let state_path = temp_state_path("tun-status");
        let backend = test_backend(&state_path);

        backend
            .apply_local_config(LocalBackendConfig {
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");
        backend
            .apply_peer_map(vec![test_peer("dev_peer", Ipv4Addr::new(100, 64, 0, 3))])
            .await
            .expect("peer map");

        let status = backend.status().await.expect("backend status");
        let message = status.message.expect("status message");

        assert!(!status.healthy);
        assert!(message.contains("crypto_peers=1"));
        assert!(message.contains("udp_bound=true"));
        assert!(message.contains("local_endpoints=1"));
        assert!(message.contains("tun_configured=false"));
        assert!(message.contains("tun_io_ready=false"));
        assert!(message.contains("TUN setup skipped"));

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn reports_local_udp_endpoint_after_local_config() {
        let state_path = temp_state_path("local-endpoint");
        let backend = test_backend(&state_path);

        backend
            .apply_local_config(LocalBackendConfig {
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");

        let endpoints = backend.local_endpoints().await.expect("local endpoints");

        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].kind, EndpointKind::Lan);
        assert_eq!(endpoints[0].protocol, TransportProtocol::Udp);
        assert_ne!(endpoints[0].port, 0);

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn shutdown_without_tun_device_succeeds() {
        let state_path = temp_state_path("shutdown-no-tun");
        let backend = test_backend(&state_path);

        backend.shutdown().await.expect("shutdown");

        cleanup_state(&state_path);
    }

    fn temp_state_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "tfscale-custom-{name}-{}.json",
            Uuid::now_v7().simple()
        ))
    }

    fn read_state(path: &Path) -> CustomBackendState {
        serde_json::from_slice(&fs::read(path).expect("state file")).expect("state json")
    }

    fn cleanup_state(path: &Path) {
        let _ = fs::remove_file(path);
    }

    fn test_backend(path: &Path) -> CustomBackend {
        CustomBackend::with_state_path("tfscale0", path).with_tun_setup(false)
    }

    fn test_public_credential() -> String {
        generate_identity().public_key
    }

    fn test_peer(device_id: &str, overlay_ip: Ipv4Addr) -> PeerConfig {
        PeerConfig {
            device_id: DeviceId::from(device_id.to_string()),
            hostname: device_id.to_string(),
            overlay_ip,
            public_credential: BackendCredential {
                public: test_public_credential(),
            },
            endpoints: Vec::new(),
            allowed_routes: vec![format!("{overlay_ip}/32")],
        }
    }
}
