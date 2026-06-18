use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tfscale_core::protocol::RelayMessage;
use tfscale_net::{
    BackendCapabilities, BackendCredential, BackendError, BackendStatus, BackendType, Endpoint,
    EndpointKind, LocalBackendConfig, NetworkBackend, PeerConfig, PeerPathDiagnostic,
    PeerPathDiagnosticKind, PublicEndpointProbe, RelayConfig, Result, TransportProtocol,
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

use crypto::{PeerCryptoSession, decode_public_credential};
use frame::{
    EncodedFrame, FRAME_TYPE_DATA, FRAME_TYPE_PROBE, FRAME_TYPE_PROBE_RESPONSE, FrameDeviceId,
};
use packet::ipv4_destination;
use transport::{TransportRuntime, select_udp_endpoint, sorted_udp_endpoints};
use tun::{TunConfig, TunDevice, TunStatus};

const STATE_VERSION: u32 = 1;
const IDENTITY_SCHEME: &str = "x25519";
const PUBLIC_CREDENTIAL_PREFIX: &str = "tfpk1_";
const DIRECT_PROBE_INTERVAL: Duration = Duration::from_secs(5);
const DIRECT_FAST_PROBE_INTERVAL: Duration = Duration::from_millis(250);
const DIRECT_FAST_PROBE_BURST_ATTEMPTS: u8 = 8;
const DIRECT_PROBE_STALE_AFTER: Duration = Duration::from_secs(15);
const DIRECT_PROBE_FAILURE_THRESHOLD: u32 = 3;

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
        let transport_tasks = runtime.transport_tasks.take();
        *runtime = RuntimeState::from_persisted(state);
        runtime.tun_status = tun_status;
        runtime.tun_device = tun_device;
        runtime.transport = transport;
        runtime.transport_tasks = transport_tasks;
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
        drop(runtime);
        self.start_transport_tasks()?;
        Ok(())
    }

    fn bind_transport(&self, listen_port: u16) -> Result<()> {
        self.stop_transport_tasks()?;
        self.state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport
            .take();
        let transport = TransportRuntime::bind(listen_port)?;
        {
            let mut runtime = self
                .state
                .lock()
                .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
            runtime.transport = Some(transport);
        }
        self.start_transport_tasks()?;
        Ok(())
    }

    fn start_transport_tasks(&self) -> Result<()> {
        let mut runtime = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        if runtime.transport_tasks.is_some()
            || runtime.tun_device.is_none()
            || runtime.transport.is_none()
        {
            return Ok(());
        }

        runtime.transport_tasks = Some(TransportTasks::start(Arc::clone(&self.state)));
        Ok(())
    }

    fn stop_transport_tasks(&self) -> Result<()> {
        let tasks = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport_tasks
            .take();
        if let Some(tasks) = tasks {
            tasks.stop();
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn send_overlay_packet(&self, packet: &[u8]) -> Result<usize> {
        self.state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .send_overlay_packet(packet)
    }

    #[allow(dead_code)]
    fn receive_overlay_packet(&self, buffer: &mut [u8]) -> Result<Option<Vec<u8>>> {
        self.state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .receive_overlay_packet(buffer)
    }

    #[allow(dead_code)]
    fn probe_peer_direct_paths(&self) -> Result<usize> {
        self.state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .probe_peer_direct_paths()
    }

    fn probe_public_endpoint_blocking(
        &self,
        probe_server: SocketAddr,
        timeout: Duration,
    ) -> Result<Option<PublicEndpointProbe>> {
        self.stop_transport_tasks()?;
        let result = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport
            .as_mut()
            .ok_or_else(|| BackendError::CommandFailed("UDP transport is not bound".to_string()))?
            .probe_public_endpoint(probe_server, timeout);
        let restart_result = self.start_transport_tasks();

        match (result, restart_result) {
            (Ok(probe), Ok(())) => Ok(probe),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) | (Err(_), Err(error)) => Err(error),
        }
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

struct RuntimeState {
    persisted: CustomBackendState,
    peers_by_device: HashMap<String, StoredPeerSession>,
    peers_by_overlay_ip: HashMap<Ipv4Addr, String>,
    peer_crypto_material_by_device: HashMap<String, PeerCryptoMaterial>,
    peer_udp_endpoints_by_device: HashMap<String, Endpoint>,
    peer_device_by_frame_id: HashMap<FrameDeviceId, String>,
    peer_transport_sessions_by_device: HashMap<String, PeerTransportSession>,
    peer_path_state_by_device: HashMap<String, PeerPathState>,
    relays: Vec<RelayConfig>,
    tun_status: Option<TunStatus>,
    tun_device: Option<TunDevice>,
    transport: Option<TransportRuntime>,
    transport_tasks: Option<TransportTasks>,
    relay_sender: Box<dyn RelayFrameSender>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            persisted: CustomBackendState::default(),
            peers_by_device: HashMap::new(),
            peers_by_overlay_ip: HashMap::new(),
            peer_crypto_material_by_device: HashMap::new(),
            peer_udp_endpoints_by_device: HashMap::new(),
            peer_device_by_frame_id: HashMap::new(),
            peer_transport_sessions_by_device: HashMap::new(),
            peer_path_state_by_device: HashMap::new(),
            relays: Vec::new(),
            tun_status: None,
            tun_device: None,
            transport: None,
            transport_tasks: None,
            relay_sender: Box::new(NullRelayFrameSender),
        }
    }
}

trait RelayFrameSender: Send + Sync {
    fn send_frame(&self, destination_device_id: &str, message: RelayMessage) -> Result<usize>;
}

struct NullRelayFrameSender;

impl RelayFrameSender for NullRelayFrameSender {
    fn send_frame(&self, _destination_device_id: &str, _message: RelayMessage) -> Result<usize> {
        Err(BackendError::CommandFailed(
            "relay transport is not connected".to_string(),
        ))
    }
}

struct TcpRelayFrameSender {
    relay: RelayConfig,
    device_id: String,
    node_key: String,
    writer: Mutex<Option<TcpStream>>,
}

impl TcpRelayFrameSender {
    fn new(relay: RelayConfig, device_id: String, node_key: String) -> Self {
        Self {
            relay,
            device_id,
            node_key,
            writer: Mutex::new(None),
        }
    }

    fn ensure_connected(&self) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        if writer.is_some() {
            return Ok(());
        }

        let mut stream = TcpStream::connect(relay_tcp_addr(&self.relay.url)?)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        write_relay_message(
            &mut stream,
            &RelayMessage::Register {
                device_id: self.device_id.clone(),
                node_key: self.node_key.clone(),
            },
        )?;
        *writer = Some(stream);
        Ok(())
    }
}

impl RelayFrameSender for TcpRelayFrameSender {
    fn send_frame(&self, _destination_device_id: &str, message: RelayMessage) -> Result<usize> {
        self.ensure_connected()?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        let writer = writer.as_mut().ok_or_else(|| {
            BackendError::CommandFailed("relay transport is not connected".to_string())
        })?;
        write_relay_message(writer, &message)?;

        let RelayMessage::Frame { payload, .. } = message else {
            return Ok(0);
        };
        Ok(payload.len())
    }
}

fn write_relay_message(writer: &mut TcpStream, message: &RelayMessage) -> Result<()> {
    let mut line = serde_json::to_vec(message)
        .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
    line.push(b'\n');
    writer
        .write_all(&line)
        .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
    Ok(())
}

fn relay_tcp_addr(url: &str) -> Result<&str> {
    url.strip_prefix("tcp://")
        .ok_or_else(|| BackendError::CommandFailed(format!("unsupported relay URL: {url}")))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PeerCryptoMaterial {
    #[allow(dead_code)]
    public_key: [u8; 32],
}

struct PeerTransportSession {
    #[allow(dead_code)]
    overlay_ip: Ipv4Addr,
    #[allow(dead_code)]
    endpoint: Endpoint,
    #[allow(dead_code)]
    crypto: PeerCryptoSession,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PeerPathKind {
    Unknown,
    Direct,
    #[allow(dead_code)]
    Relay,
}

#[derive(Clone, Debug)]
struct PeerPathState {
    current_path: PeerPathKind,
    active_endpoint: Option<Endpoint>,
    last_probe_at: Option<SystemTime>,
    last_success_at: Option<SystemTime>,
    last_probe_sent_at_unix_millis: Option<u128>,
    fast_probe_attempts_remaining: u8,
    failures: u32,
    rtt_ms: Option<u64>,
}

impl Default for PeerPathState {
    fn default() -> Self {
        Self {
            current_path: PeerPathKind::Unknown,
            active_endpoint: None,
            last_probe_at: None,
            last_success_at: None,
            last_probe_sent_at_unix_millis: None,
            fast_probe_attempts_remaining: DIRECT_FAST_PROBE_BURST_ATTEMPTS,
            failures: 0,
            rtt_ms: None,
        }
    }
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
        let peer_device_by_frame_id = persisted
            .peers
            .iter()
            .filter_map(|peer| {
                FrameDeviceId::from_device_id(&peer.device_id)
                    .ok()
                    .map(|frame_id| (frame_id, peer.device_id.clone()))
            })
            .collect();
        let peer_transport_sessions_by_device =
            build_peer_transport_sessions(&persisted).unwrap_or_default();
        let peer_path_state_by_device = persisted
            .peers
            .iter()
            .map(|peer| (peer.device_id.clone(), PeerPathState::default()))
            .collect();

        Self {
            persisted,
            peers_by_device,
            peers_by_overlay_ip,
            peer_crypto_material_by_device,
            peer_udp_endpoints_by_device,
            peer_device_by_frame_id,
            peer_transport_sessions_by_device,
            peer_path_state_by_device,
            relays: Vec::new(),
            tun_status: None,
            tun_device: None,
            transport: None,
            transport_tasks: None,
            relay_sender: Box::new(NullRelayFrameSender),
        }
    }

    fn send_overlay_packet(&mut self, packet: &[u8]) -> Result<usize> {
        let destination = ipv4_destination(packet)?;
        let device_id = self
            .peers_by_overlay_ip
            .get(&destination)
            .ok_or_else(|| {
                BackendError::CommandFailed(format!(
                    "no peer session for overlay destination: {destination}"
                ))
            })?
            .clone();
        let relay_path = self.peer_path_is_relay(&device_id);
        let session = self
            .peer_transport_sessions_by_device
            .get_mut(&device_id)
            .ok_or_else(|| {
                BackendError::CommandFailed(format!(
                    "peer transport session is not ready: {device_id}"
                ))
            })?;
        let frame = session.crypto.seal(packet)?;
        if relay_path {
            return self.relay_sender.send_frame(
                &device_id,
                RelayMessage::Frame {
                    source_device_id: self
                        .persisted
                        .local_config
                        .as_ref()
                        .map(|config| config.device_id.clone())
                        .unwrap_or_default(),
                    destination_device_id: device_id.clone(),
                    payload: URL_SAFE_NO_PAD.encode(&frame),
                },
            );
        }

        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| BackendError::CommandFailed("UDP transport is not bound".to_string()))?;
        transport.send_frame(&session.endpoint, &frame)
    }

    fn peer_path_is_relay(&self, device_id: &str) -> bool {
        self.peer_path_state_by_device
            .get(device_id)
            .map(|state| state.current_path == PeerPathKind::Relay)
            .unwrap_or(false)
    }

    fn probe_peer_direct_paths(&mut self) -> Result<usize> {
        let mut sent = 0;
        self.expire_stale_direct_paths(SystemTime::now());
        let Some(transport) = self.transport.as_mut() else {
            return Ok(0);
        };

        for peer in &self.persisted.peers {
            let path_state = self
                .peer_path_state_by_device
                .entry(peer.device_id.clone())
                .or_default();
            if !peer_path_probe_due(path_state, SystemTime::now()) {
                continue;
            }

            let Some(session) = self
                .peer_transport_sessions_by_device
                .get_mut(&peer.device_id)
            else {
                continue;
            };
            let sent_at = now_unix_millis();
            for endpoint in sorted_udp_endpoints(&peer.endpoints) {
                let frame = session
                    .crypto
                    .seal_message(format!("probe {sent_at}").as_bytes(), FRAME_TYPE_PROBE)?;
                transport.send_frame(&endpoint, &frame)?;
                sent += 1;
            }
            path_state.last_probe_at = Some(SystemTime::now());
            path_state.last_probe_sent_at_unix_millis = Some(sent_at);
            if path_state.fast_probe_attempts_remaining > 0 {
                path_state.fast_probe_attempts_remaining -= 1;
            }
            if path_state.current_path != PeerPathKind::Direct {
                path_state.failures = path_state.failures.saturating_add(1);
            }
        }

        Ok(sent)
    }

    fn receive_overlay_packet(&mut self, buffer: &mut [u8]) -> Result<Option<Vec<u8>>> {
        let Some((received, source_addr)) = self
            .transport
            .as_mut()
            .ok_or_else(|| BackendError::CommandFailed("UDP transport is not bound".to_string()))?
            .receive_frame(buffer)?
        else {
            return Ok(None);
        };

        self.open_received_frame(&buffer[..received], Some(source_addr))
    }

    fn open_relay_frame(&mut self, message: RelayMessage) -> Result<Option<Vec<u8>>> {
        let RelayMessage::Frame { payload, .. } = message else {
            return Ok(None);
        };
        let frame = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        self.open_received_frame(&frame, None)
    }

    fn open_received_frame(
        &mut self,
        frame: &[u8],
        source_addr: Option<SocketAddr>,
    ) -> Result<Option<Vec<u8>>> {
        let decoded = match EncodedFrame::decode(frame) {
            Ok(decoded) => decoded,
            Err(error) => {
                if let Some(transport) = self.transport.as_mut() {
                    transport.record_rx_drop();
                }
                return Err(error);
            }
        };
        let device_id = self
            .peer_device_by_frame_id
            .get(&decoded.header.source)
            .ok_or_else(|| {
                if let Some(transport) = self.transport.as_mut() {
                    transport.record_rx_drop();
                }
                BackendError::CommandFailed("missing peer session for frame source".to_string())
            })?
            .clone();
        let session = self
            .peer_transport_sessions_by_device
            .get_mut(&device_id)
            .ok_or_else(|| {
                if let Some(transport) = self.transport.as_mut() {
                    transport.record_rx_drop();
                }
                BackendError::CommandFailed(format!(
                    "peer transport session is not ready: {device_id}"
                ))
            })?;
        let opened = match session.crypto.open_message(frame) {
            Ok(opened) => opened,
            Err(error) => {
                if let Some(transport) = self.transport.as_mut() {
                    transport.record_rx_drop();
                }
                return Err(error);
            }
        };

        match opened.message_type {
            FRAME_TYPE_DATA => Ok(Some(opened.plaintext)),
            FRAME_TYPE_PROBE => {
                let Some(source_addr) = source_addr else {
                    return Ok(None);
                };
                let response_payload = encode_probe_response_payload(&opened.plaintext);
                let response = session
                    .crypto
                    .seal_message(&response_payload, FRAME_TYPE_PROBE_RESPONSE)?;
                let endpoint = Endpoint {
                    kind: EndpointKind::Public,
                    address: source_addr.ip(),
                    port: source_addr.port(),
                    protocol: TransportProtocol::Udp,
                };
                self.transport
                    .as_mut()
                    .ok_or_else(|| {
                        BackendError::CommandFailed("UDP transport is not bound".to_string())
                    })?
                    .send_frame(&endpoint, &response)?;
                self.mark_peer_direct(&device_id, endpoint, None);
                Ok(None)
            }
            FRAME_TYPE_PROBE_RESPONSE => {
                let Some(source_addr) = source_addr else {
                    return Ok(None);
                };
                let endpoint = Endpoint {
                    kind: EndpointKind::Public,
                    address: source_addr.ip(),
                    port: source_addr.port(),
                    protocol: TransportProtocol::Udp,
                };
                let rtt_ms = probe_response_rtt_ms(&opened.plaintext, now_unix_millis());
                self.mark_peer_direct(&device_id, endpoint, rtt_ms);
                Ok(None)
            }
            other => Err(BackendError::CommandFailed(format!(
                "unsupported opened frame message type: {other}"
            ))),
        }
    }

    fn mark_peer_direct(&mut self, device_id: &str, endpoint: Endpoint, rtt_ms: Option<u64>) {
        let state = self
            .peer_path_state_by_device
            .entry(device_id.to_string())
            .or_default();
        state.current_path = PeerPathKind::Direct;
        state.active_endpoint = Some(endpoint.clone());
        state.last_success_at = Some(SystemTime::now());
        state.failures = 0;
        state.rtt_ms = rtt_ms;
        state.fast_probe_attempts_remaining = 0;

        if let Some(session) = self.peer_transport_sessions_by_device.get_mut(device_id) {
            session.endpoint = endpoint;
        }
    }

    fn expire_stale_direct_paths(&mut self, now: SystemTime) {
        for state in self.peer_path_state_by_device.values_mut() {
            if state.current_path != PeerPathKind::Direct {
                continue;
            }

            let stale = state
                .last_success_at
                .and_then(|last_success| now.duration_since(last_success).ok())
                .map(|elapsed| elapsed >= DIRECT_PROBE_STALE_AFTER)
                .unwrap_or(true);
            if stale {
                state.failures = state.failures.saturating_add(1);
            }
            if state.failures >= DIRECT_PROBE_FAILURE_THRESHOLD {
                if self.relays.is_empty() {
                    state.current_path = PeerPathKind::Unknown;
                    state.active_endpoint = None;
                } else {
                    state.current_path = PeerPathKind::Relay;
                    state.active_endpoint = Some(relay_endpoint(&self.relays[0]));
                }
                state.rtt_ms = None;
                state.fast_probe_attempts_remaining = DIRECT_FAST_PROBE_BURST_ATTEMPTS;
            }
        }
    }

    fn try_read_tun_packet(&mut self, buffer: &mut [u8]) -> Result<Option<usize>> {
        let Some(device) = self.tun_device.as_ref() else {
            return Ok(None);
        };
        device.try_read_packet(buffer)
    }

    fn write_tun_packet(&mut self, packet: &[u8]) -> Result<usize> {
        let Some(device) = self.tun_device.as_ref() else {
            return Err(BackendError::CommandFailed(
                "TUN device is not configured".to_string(),
            ));
        };
        device.write_packet(packet)
    }

    fn record_rx_drop(&mut self) {
        if let Some(transport) = self.transport.as_mut() {
            transport.record_rx_drop();
        }
    }
}

struct TransportTasks {
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

impl TransportTasks {
    fn start(state: Arc<Mutex<RuntimeState>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let mut handles = vec![
            spawn_tun_to_udp_loop(Arc::clone(&state), Arc::clone(&stop)),
            spawn_udp_to_tun_loop(Arc::clone(&state), Arc::clone(&stop)),
        ];
        if let Some(handle) = spawn_relay_to_tun_loop(state, Arc::clone(&stop)) {
            handles.push(handle);
        }

        Self { stop, handles }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

fn spawn_tun_to_udp_loop(state: Arc<Mutex<RuntimeState>>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = vec![0u8; 1500];
        while !stop.load(Ordering::SeqCst) {
            let packet = {
                let mut runtime = match state.lock() {
                    Ok(runtime) => runtime,
                    Err(_) => break,
                };
                match runtime.try_read_tun_packet(&mut buffer) {
                    Ok(Some(size)) => Some(buffer[..size].to_vec()),
                    Ok(None) => None,
                    Err(_) => {
                        runtime.record_rx_drop();
                        None
                    }
                }
            };

            if let Some(packet) = packet {
                if let Ok(mut runtime) = state.lock() {
                    let _ = runtime.send_overlay_packet(&packet);
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    })
}

fn spawn_udp_to_tun_loop(state: Arc<Mutex<RuntimeState>>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = vec![0u8; 1500];
        while !stop.load(Ordering::SeqCst) {
            let packet = {
                let mut runtime = match state.lock() {
                    Ok(runtime) => runtime,
                    Err(_) => break,
                };
                match runtime.receive_overlay_packet(&mut buffer) {
                    Ok(packet) => packet,
                    Err(_) => {
                        runtime.record_rx_drop();
                        None
                    }
                }
            };

            if let Some(packet) = packet {
                if let Ok(mut runtime) = state.lock() {
                    if runtime.write_tun_packet(&packet).is_err() {
                        runtime.record_rx_drop();
                    }
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    })
}

fn spawn_relay_to_tun_loop(
    state: Arc<Mutex<RuntimeState>>,
    stop: Arc<AtomicBool>,
) -> Option<JoinHandle<()>> {
    let (relay, device_id) = {
        let runtime = state.lock().ok()?;
        let relay = runtime.relays.first()?.clone();
        let device_id = runtime.persisted.local_config.as_ref()?.device_id.clone();
        (relay, device_id)
    };

    Some(thread::spawn(move || {
        let mut stream = match TcpStream::connect(match relay_tcp_addr(&relay.url) {
            Ok(addr) => addr,
            Err(_) => return,
        }) {
            Ok(stream) => stream,
            Err(_) => return,
        };
        if write_relay_message(
            &mut stream,
            &RelayMessage::Register {
                device_id,
                node_key: String::new(),
            },
        )
        .is_err()
        {
            return;
        }

        let mut reader = BufReader::new(stream);
        while !stop.load(Ordering::SeqCst) {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let packet = match serde_json::from_str::<RelayMessage>(&line) {
                        Ok(message) => match state.lock() {
                            Ok(mut runtime) => match runtime.open_relay_frame(message) {
                                Ok(packet) => packet,
                                Err(_) => {
                                    runtime.record_rx_drop();
                                    None
                                }
                            },
                            Err(_) => break,
                        },
                        Err(_) => None,
                    };
                    if let Some(packet) = packet {
                        if let Ok(mut runtime) = state.lock() {
                            if runtime.write_tun_packet(&packet).is_err() {
                                runtime.record_rx_drop();
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
    }))
}

fn build_peer_transport_sessions(
    persisted: &CustomBackendState,
) -> Result<HashMap<String, PeerTransportSession>> {
    let Some(identity) = persisted.identity.as_ref() else {
        return Ok(HashMap::new());
    };
    let Some(local_config) = persisted.local_config.as_ref() else {
        return Ok(HashMap::new());
    };

    let local_id = FrameDeviceId::from_device_id(&local_config.device_id)?;
    let local_private_key = decode_key(&identity.private_key)?;
    let mut sessions = HashMap::new();

    for peer in &persisted.peers {
        let peer_id = FrameDeviceId::from_device_id(&peer.device_id)?;
        let Some(endpoint) = select_udp_endpoint(&peer.endpoints) else {
            continue;
        };
        let crypto = PeerCryptoSession::new(
            local_private_key,
            &identity.public_key,
            &peer.public_key,
            local_id,
            peer_id,
        )?;
        sessions.insert(
            peer.device_id.clone(),
            PeerTransportSession {
                overlay_ip: peer.overlay_ip,
                endpoint,
                crypto,
            },
        );
    }

    Ok(sessions)
}

fn peer_path_probe_due(state: &PeerPathState, now: SystemTime) -> bool {
    let interval =
        if state.current_path != PeerPathKind::Direct && state.fast_probe_attempts_remaining > 0 {
            DIRECT_FAST_PROBE_INTERVAL
        } else {
            DIRECT_PROBE_INTERVAL
        };

    state
        .last_probe_at
        .and_then(|last_probe| now.duration_since(last_probe).ok())
        .map(|elapsed| elapsed >= interval)
        .unwrap_or(true)
}

fn encode_probe_response_payload(probe_payload: &[u8]) -> Vec<u8> {
    let timestamp = probe_payload
        .strip_prefix(b"probe ")
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or_else(now_unix_millis);
    format!("probe_response {timestamp}").into_bytes()
}

fn probe_response_rtt_ms(response_payload: &[u8], now_unix_millis: u128) -> Option<u64> {
    let timestamp = response_payload
        .strip_prefix(b"probe_response ")
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<u128>().ok())?;
    now_unix_millis
        .checked_sub(timestamp)
        .and_then(|elapsed| elapsed.try_into().ok())
}

fn direct_path_status(peer_paths: &HashMap<String, PeerPathState>) -> String {
    let mut paths = peer_paths
        .iter()
        .filter_map(|(device_id, state)| {
            if state.current_path != PeerPathKind::Direct {
                return None;
            }
            let endpoint = state.active_endpoint.as_ref()?;
            let rtt = state
                .rtt_ms
                .map(|value| format!("{value}ms"))
                .unwrap_or_else(|| "-".to_string());
            Some(format!(
                "{}@{}:{}/rtt={}",
                device_id, endpoint.address, endpoint.port, rtt
            ))
        })
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        "none".to_string()
    } else {
        paths.join(",")
    }
}

fn fast_probe_status(peer_paths: &HashMap<String, PeerPathState>) -> String {
    let count = peer_paths
        .values()
        .filter(|state| {
            state.current_path != PeerPathKind::Direct && state.fast_probe_attempts_remaining > 0
        })
        .count();
    count.to_string()
}

fn peer_path_diagnostics(
    peers: &[StoredPeerSession],
    peer_paths: &HashMap<String, PeerPathState>,
    tx_packets: u64,
    rx_packets: u64,
) -> Vec<PeerPathDiagnostic> {
    peers
        .iter()
        .map(|peer| {
            let state = peer_paths.get(&peer.device_id);
            PeerPathDiagnostic {
                device_id: peer.device_id.clone(),
                path: state
                    .map(|state| diagnostic_kind(&state.current_path))
                    .unwrap_or(PeerPathDiagnosticKind::Unknown),
                endpoint: state.and_then(|state| {
                    state
                        .active_endpoint
                        .as_ref()
                        .map(|endpoint| format!("{}:{}", endpoint.address, endpoint.port))
                }),
                rtt_ms: state.and_then(|state| state.rtt_ms),
                failures: state.map(|state| state.failures).unwrap_or_default(),
                tx_packets,
                rx_packets,
            }
        })
        .collect()
}

fn diagnostic_kind(value: &PeerPathKind) -> PeerPathDiagnosticKind {
    match value {
        PeerPathKind::Unknown => PeerPathDiagnosticKind::Unknown,
        PeerPathKind::Direct => PeerPathDiagnosticKind::Direct,
        PeerPathKind::Relay => PeerPathDiagnosticKind::Relay,
    }
}

fn relay_endpoint(_relay: &RelayConfig) -> Endpoint {
    Endpoint {
        kind: EndpointKind::Relay,
        address: IpAddr::from(Ipv4Addr::UNSPECIFIED),
        port: 0,
        protocol: TransportProtocol::Tcp,
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

fn now_unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
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
            supports_nat_traversal: true,
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
        if !self.tun_setup_enabled {
            self.bind_transport(tun_config.listen_port)?;
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
                self.bind_transport(tun_config.listen_port)?;
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

    async fn apply_relay_map(&self, relays: Vec<RelayConfig>) -> Result<()> {
        let mut runtime = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        runtime.relay_sender = runtime
            .persisted
            .local_config
            .as_ref()
            .and_then(|config| {
                relays.first().cloned().map(|relay| {
                    Box::new(TcpRelayFrameSender::new(
                        relay,
                        config.device_id.clone(),
                        String::new(),
                    )) as Box<dyn RelayFrameSender>
                })
            })
            .unwrap_or_else(|| Box::new(NullRelayFrameSender));
        runtime.relays = relays;
        Ok(())
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

    async fn probe_public_endpoint(
        &self,
        probe_server: SocketAddr,
        timeout: Duration,
    ) -> Result<Option<PublicEndpointProbe>> {
        self.probe_public_endpoint_blocking(probe_server, timeout)
    }

    async fn maintain_peer_paths(&self) -> Result<()> {
        self.probe_peer_direct_paths()?;
        Ok(())
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
        let transport_running = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .transport_tasks
            .is_some();
        let transport_peers = runtime.peer_udp_endpoints_by_device.len();
        let transport_sessions = runtime.peer_transport_sessions_by_device.len();
        let peer_path_state_by_device = self
            .state
            .lock()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?
            .peer_path_state_by_device
            .clone();
        let peer_diagnostics = peer_path_diagnostics(
            &runtime.persisted.peers,
            &peer_path_state_by_device,
            transport_status.tx_packets,
            transport_status.rx_packets,
        );
        let direct_peers = peer_path_state_by_device
            .values()
            .filter(|state| state.current_path == PeerPathKind::Direct)
            .count();
        let relay_peers = peer_path_state_by_device
            .values()
            .filter(|state| state.current_path == PeerPathKind::Relay)
            .count();
        let direct_paths = direct_path_status(&peer_path_state_by_device);
        let fast_probe_peers = fast_probe_status(&peer_path_state_by_device);
        let reachable_peers = direct_peers + relay_peers;
        Ok(BackendStatus {
            backend_type: self.backend_type(),
            interface_name: self.interface_name.clone(),
            healthy: tun_configured
                && tun_io_ready
                && transport_status.udp_bound
                && transport_running,
            message: Some(format!(
                "custom backend state: credentials={} local_config={} peers={} peer_indexes=device:{} overlay:{} crypto_peers={} udp_bound={} transport_running={} local_endpoints={} transport_peers={} transport_sessions={} direct_peers={} relay_peers={} fast_probe_peers={} direct_paths={} reachable_peers={} tx_packets={} rx_packets={} tx_drops={} rx_drops={} tun_configured={} tun_io_ready={} tun_message={} data_plane=transport_ready",
                has_credentials,
                has_local_config,
                runtime.persisted.peers.len(),
                runtime.peers_by_device.len(),
                runtime.peers_by_overlay_ip.len(),
                runtime.peer_crypto_material_by_device.len(),
                transport_status.udp_bound,
                transport_running,
                transport_status.local_endpoints,
                transport_peers,
                transport_sessions,
                direct_peers,
                relay_peers,
                fast_probe_peers,
                direct_paths,
                reachable_peers,
                transport_status.tx_packets,
                transport_status.rx_packets,
                transport_status.tx_drops,
                transport_status.rx_drops,
                tun_configured,
                tun_io_ready,
                tun_message
            )),
            peers: peer_diagnostics,
        })
    }

    async fn shutdown(&self) -> Result<()> {
        self.stop_transport_tasks()?;
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
                device_id: DeviceId::new().to_string(),
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
                device_id: DeviceId::new().to_string(),
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
        assert!(message.contains("transport_running=false"));
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
                device_id: DeviceId::new().to_string(),
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
    async fn probes_public_endpoint_with_backend_udp_socket() {
        let state_path = temp_state_path("public-probe");
        let backend = test_backend(&state_path);
        backend
            .apply_local_config(LocalBackendConfig {
                device_id: DeviceId::new().to_string(),
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");
        let local_endpoint = backend
            .local_endpoints()
            .await
            .expect("local endpoints")
            .into_iter()
            .next()
            .expect("local endpoint");
        let server =
            std::net::UdpSocket::bind(std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .expect("server");
        server.set_nonblocking(true).expect("nonblocking server");
        let server_addr = server.local_addr().expect("server addr");
        let handle = std::thread::spawn(move || {
            let mut buffer = [0u8; 256];
            for _ in 0..10 {
                match server.recv_from(&mut buffer) {
                    Ok((_received, source)) => {
                        let response = transport::encode_probe_response(source);
                        server.send_to(&response, source).expect("send response");
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("server receive failed: {error}"),
                }
            }
            panic!("probe request not received");
        });

        let probe = backend
            .probe_public_endpoint(server_addr, Duration::from_secs(1))
            .await
            .expect("probe")
            .expect("observed endpoint");
        handle.join().expect("server thread");

        assert_eq!(probe.observed_endpoint.kind, EndpointKind::Public);
        assert_eq!(probe.observed_endpoint.port, local_endpoint.port);
        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn builds_transport_session_for_peer_with_udp_endpoint() {
        let state_path = temp_state_path("transport-session");
        let backend = test_backend(&state_path);
        let local_device_id = DeviceId::new().to_string();
        let peer_device_id = DeviceId::new().to_string();

        backend
            .ensure_credentials()
            .await
            .expect("backend credentials");
        backend
            .apply_local_config(LocalBackendConfig {
                device_id: local_device_id,
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");
        backend
            .apply_peer_map(vec![test_peer_with_endpoint(
                &peer_device_id,
                Ipv4Addr::new(100, 64, 0, 3),
                endpoint(Ipv4Addr::LOCALHOST, 51820),
            )])
            .await
            .expect("peer map");

        let status = backend.status().await.expect("backend status");
        let message = status.message.expect("status message");

        assert!(message.contains("transport_peers=1"));
        assert!(message.contains("transport_sessions=1"));

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn sends_overlay_packet_to_peer_udp_endpoint() {
        let state_path = temp_state_path("send-overlay-packet");
        let backend = test_backend(&state_path);
        let mut peer_receiver = TransportRuntime::bind(0).expect("peer transport");
        let local_device_id = DeviceId::new().to_string();
        let peer_device_id = DeviceId::new().to_string();

        backend
            .ensure_credentials()
            .await
            .expect("backend credentials");
        backend
            .apply_local_config(LocalBackendConfig {
                device_id: local_device_id,
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");
        backend
            .apply_peer_map(vec![test_peer_with_endpoint(
                &peer_device_id,
                Ipv4Addr::new(100, 64, 0, 3),
                endpoint(
                    Ipv4Addr::LOCALHOST,
                    peer_receiver.local_addr().expect("peer addr").port(),
                ),
            )])
            .await
            .expect("peer map");

        let sent = backend
            .send_overlay_packet(&ipv4_packet(
                Ipv4Addr::new(100, 64, 0, 2),
                Ipv4Addr::new(100, 64, 0, 3),
            ))
            .expect("send overlay packet");

        assert!(sent > 0);
        let mut buffer = [0u8; 1500];
        let received = receive_with_retry(&mut peer_receiver, &mut buffer).expect("received frame");
        assert_eq!(received, sent);
        let status = backend.status().await.expect("backend status");
        let message = status.message.expect("status message");
        assert!(message.contains("tx_packets=1"));

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn sends_overlay_packet_to_relay_when_peer_path_is_relay() {
        let state_path = temp_state_path("send-overlay-relay");
        let backend = test_backend(&state_path);
        let local_device_id = DeviceId::new().to_string();
        let peer_device_id = DeviceId::new().to_string();
        let sent_frames = Arc::new(Mutex::new(Vec::new()));

        backend
            .ensure_credentials()
            .await
            .expect("backend credentials");
        backend
            .apply_local_config(LocalBackendConfig {
                device_id: local_device_id.clone(),
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");
        backend
            .apply_peer_map(vec![test_peer_with_endpoint(
                &peer_device_id,
                Ipv4Addr::new(100, 64, 0, 3),
                endpoint(Ipv4Addr::LOCALHOST, 51820),
            )])
            .await
            .expect("peer map");
        {
            let mut runtime = backend.state.lock().expect("runtime");
            runtime.relay_sender = Box::new(TestRelayFrameSender {
                sent: Arc::clone(&sent_frames),
            });
            runtime.peer_path_state_by_device.insert(
                peer_device_id.clone(),
                PeerPathState {
                    current_path: PeerPathKind::Relay,
                    active_endpoint: Some(relay_endpoint(&RelayConfig {
                        relay_id: "relay_1".to_string(),
                        url: "tcp://127.0.0.1:9443".to_string(),
                        region: "local".to_string(),
                    })),
                    ..PeerPathState::default()
                },
            );
        }

        let sent = backend
            .send_overlay_packet(&ipv4_packet(
                Ipv4Addr::new(100, 64, 0, 2),
                Ipv4Addr::new(100, 64, 0, 3),
            ))
            .expect("send relay packet");

        let frames = sent_frames.lock().expect("sent frames");
        assert_eq!(frames.len(), 1);
        assert_eq!(sent, frames[0].payload.len());
        assert_eq!(frames[0].source_device_id, local_device_id);
        assert_eq!(frames[0].destination_device_id, peer_device_id);
        assert!(!frames[0].payload.is_empty());

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn opens_overlay_packet_from_relay_frame() {
        let left_state_path = temp_state_path("relay-open-left");
        let right_state_path = temp_state_path("relay-open-right");
        let left = test_backend(&left_state_path);
        let right = test_backend(&right_state_path);
        let left_device_id = DeviceId::new().to_string();
        let right_device_id = DeviceId::new().to_string();
        let left_credential = left.ensure_credentials().await.expect("left credentials");
        let right_credential = right.ensure_credentials().await.expect("right credentials");

        left.apply_local_config(LocalBackendConfig {
            device_id: left_device_id.clone(),
            interface_name: "tfscale0".to_string(),
            overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
            listen_port: 0,
        })
        .await
        .expect("left local config");
        right
            .apply_local_config(LocalBackendConfig {
                device_id: right_device_id.clone(),
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 3),
                listen_port: 0,
            })
            .await
            .expect("right local config");
        left.apply_peer_map(vec![PeerConfig {
            public_credential: right_credential,
            endpoints: vec![endpoint(Ipv4Addr::LOCALHOST, 51820)],
            ..test_peer(&right_device_id, Ipv4Addr::new(100, 64, 0, 3))
        }])
        .await
        .expect("left peer map");
        right
            .apply_peer_map(vec![PeerConfig {
                public_credential: left_credential,
                endpoints: vec![endpoint(Ipv4Addr::LOCALHOST, 51820)],
                ..test_peer(&left_device_id, Ipv4Addr::new(100, 64, 0, 2))
            }])
            .await
            .expect("right peer map");

        let frame = {
            let mut runtime = left.state.lock().expect("left runtime");
            let session = runtime
                .peer_transport_sessions_by_device
                .get_mut(&right_device_id)
                .expect("right session");
            session
                .crypto
                .seal(&ipv4_packet(
                    Ipv4Addr::new(100, 64, 0, 2),
                    Ipv4Addr::new(100, 64, 0, 3),
                ))
                .expect("sealed frame")
        };
        let message = RelayMessage::Frame {
            source_device_id: left_device_id,
            destination_device_id: right_device_id,
            payload: URL_SAFE_NO_PAD.encode(frame),
        };

        let packet = right
            .state
            .lock()
            .expect("right runtime")
            .open_relay_frame(message)
            .expect("open relay frame")
            .expect("packet");

        assert_eq!(
            packet,
            ipv4_packet(Ipv4Addr::new(100, 64, 0, 2), Ipv4Addr::new(100, 64, 0, 3))
        );
        cleanup_state(&left_state_path);
        cleanup_state(&right_state_path);
    }

    #[test]
    fn tcp_relay_sender_registers_and_writes_frame() {
        let listener =
            std::net::TcpListener::bind(std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .expect("listener");
        let addr = listener.local_addr().expect("listener addr");
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accepted");
            let mut lines = std::io::BufReader::new(stream).lines();
            let register = lines.next().expect("register line").expect("register line");
            let frame = lines.next().expect("frame line").expect("frame line");
            (register, frame)
        });
        let sender = TcpRelayFrameSender::new(
            RelayConfig {
                relay_id: "relay_1".to_string(),
                url: format!("tcp://{addr}"),
                region: "local".to_string(),
            },
            "dev_a".to_string(),
            "node-key".to_string(),
        );

        let sent = sender
            .send_frame(
                "dev_b",
                RelayMessage::Frame {
                    source_device_id: "dev_a".to_string(),
                    destination_device_id: "dev_b".to_string(),
                    payload: "encrypted-frame".to_string(),
                },
            )
            .expect("send frame");
        let (register, frame) = handle.join().expect("server thread");
        let register: RelayMessage = serde_json::from_str(&register).expect("register json");
        let frame: RelayMessage = serde_json::from_str(&frame).expect("frame json");

        assert_eq!(sent, "encrypted-frame".len());
        assert!(matches!(
            register,
            RelayMessage::Register {
                device_id,
                node_key
            } if device_id == "dev_a" && node_key == "node-key"
        ));
        assert!(matches!(
            frame,
            RelayMessage::Frame {
                source_device_id,
                destination_device_id,
                payload
            } if source_device_id == "dev_a"
                && destination_device_id == "dev_b"
                && payload == "encrypted-frame"
        ));
    }

    #[tokio::test]
    async fn receives_overlay_packet_from_peer_udp_endpoint() {
        let state_path = temp_state_path("receive-overlay-packet");
        let backend = test_backend(&state_path);
        let local_device_id = DeviceId::new().to_string();
        let peer_device_id = DeviceId::new().to_string();
        let peer_identity = generate_identity();

        let local_credential = backend
            .ensure_credentials()
            .await
            .expect("backend credentials");
        backend
            .apply_local_config(LocalBackendConfig {
                device_id: local_device_id.clone(),
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");
        backend
            .apply_peer_map(vec![PeerConfig {
                public_credential: BackendCredential {
                    public: peer_identity.public_key.clone(),
                },
                ..test_peer_with_endpoint(
                    &peer_device_id,
                    Ipv4Addr::new(100, 64, 0, 3),
                    endpoint(Ipv4Addr::LOCALHOST, 51820),
                )
            }])
            .await
            .expect("peer map");

        let mut peer_crypto = PeerCryptoSession::new(
            decode_key(&peer_identity.private_key).expect("peer private key"),
            &peer_identity.public_key,
            &local_credential.public,
            FrameDeviceId::from_device_id(&peer_device_id).expect("peer frame id"),
            FrameDeviceId::from_device_id(&local_device_id).expect("local frame id"),
        )
        .expect("peer crypto");
        let packet = ipv4_packet(Ipv4Addr::new(100, 64, 0, 3), Ipv4Addr::new(100, 64, 0, 2));
        let frame = peer_crypto.seal(&packet).expect("sealed packet");
        let mut peer_transport = TransportRuntime::bind(0).expect("peer transport");
        let local_endpoint = backend
            .local_endpoints()
            .await
            .expect("local endpoints")
            .into_iter()
            .next()
            .expect("local endpoint");

        peer_transport
            .send_frame(&local_endpoint, &frame)
            .expect("send frame");

        let mut buffer = [0u8; 1500];
        let opened =
            receive_backend_packet_with_retry(&backend, &mut buffer).expect("received packet");

        assert_eq!(opened, packet);
        let status = backend.status().await.expect("backend status");
        let message = status.message.expect("status message");
        assert!(message.contains("rx_packets=1"));
        assert!(message.contains("rx_drops=0"));

        cleanup_state(&state_path);
    }

    #[tokio::test]
    async fn direct_probe_round_trip_marks_peer_paths_direct() {
        let left_state_path = temp_state_path("direct-probe-left");
        let right_state_path = temp_state_path("direct-probe-right");
        let left = test_backend(&left_state_path);
        let right = test_backend(&right_state_path);
        let left_device_id = DeviceId::new().to_string();
        let right_device_id = DeviceId::new().to_string();
        let left_credential = left.ensure_credentials().await.expect("left credentials");
        let right_credential = right.ensure_credentials().await.expect("right credentials");

        left.apply_local_config(LocalBackendConfig {
            device_id: left_device_id.clone(),
            interface_name: "tfscale0".to_string(),
            overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
            listen_port: 0,
        })
        .await
        .expect("left local config");
        right
            .apply_local_config(LocalBackendConfig {
                device_id: right_device_id.clone(),
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 3),
                listen_port: 0,
            })
            .await
            .expect("right local config");

        let left_endpoint = left
            .local_endpoints()
            .await
            .expect("left endpoint")
            .into_iter()
            .next()
            .expect("left endpoint");
        let right_endpoint = right
            .local_endpoints()
            .await
            .expect("right endpoint")
            .into_iter()
            .next()
            .expect("right endpoint");

        left.apply_peer_map(vec![PeerConfig {
            public_credential: right_credential,
            endpoints: vec![right_endpoint],
            ..test_peer(&right_device_id, Ipv4Addr::new(100, 64, 0, 3))
        }])
        .await
        .expect("left peer map");
        right
            .apply_peer_map(vec![PeerConfig {
                public_credential: left_credential,
                endpoints: vec![left_endpoint],
                ..test_peer(&left_device_id, Ipv4Addr::new(100, 64, 0, 2))
            }])
            .await
            .expect("right peer map");

        assert_eq!(left.probe_peer_direct_paths().expect("send probe"), 1);
        let mut buffer = [0u8; 1500];
        receive_backend_control_frame_with_retry(&right, &mut buffer).expect("right probe");
        receive_backend_control_frame_with_retry(&left, &mut buffer).expect("left response");

        let left_status = left.status().await.expect("left status");
        let right_status = right.status().await.expect("right status");
        let left_message = left_status.message.unwrap();
        assert!(left_message.contains("direct_peers=1"));
        assert!(left_message.contains("direct_paths="));
        assert!(left_message.contains("/rtt="));
        assert_eq!(left_status.peers.len(), 1);
        assert_eq!(left_status.peers[0].device_id, right_device_id);
        assert_eq!(left_status.peers[0].path, PeerPathDiagnosticKind::Direct);
        assert!(left_status.peers[0].endpoint.is_some());
        assert!(right_status.message.unwrap().contains("direct_peers=1"));

        cleanup_state(&left_state_path);
        cleanup_state(&right_state_path);
    }

    #[tokio::test]
    async fn direct_probe_consumes_fast_probe_attempt() {
        let state_path = temp_state_path("direct-fast-probe-consume");
        let backend = test_backend(&state_path);
        let local_device_id = DeviceId::new().to_string();
        let peer_device_id = DeviceId::new().to_string();
        let peer_identity = generate_identity();

        let local_credential = backend
            .ensure_credentials()
            .await
            .expect("backend credentials");
        backend
            .apply_local_config(LocalBackendConfig {
                device_id: local_device_id,
                interface_name: "tfscale0".to_string(),
                overlay_ip: Ipv4Addr::new(100, 64, 0, 2),
                listen_port: 0,
            })
            .await
            .expect("local config");
        backend
            .apply_peer_map(vec![PeerConfig {
                public_credential: BackendCredential {
                    public: peer_identity.public_key,
                },
                ..test_peer_with_endpoint(
                    &peer_device_id,
                    Ipv4Addr::new(100, 64, 0, 3),
                    endpoint(Ipv4Addr::LOCALHOST, 51820),
                )
            }])
            .await
            .expect("peer map");

        assert_eq!(backend.probe_peer_direct_paths().expect("probe"), 1);
        let runtime = backend.state.lock().expect("runtime");
        let state = runtime
            .peer_path_state_by_device
            .get(&peer_device_id)
            .expect("peer path");
        assert_eq!(
            state.fast_probe_attempts_remaining,
            DIRECT_FAST_PROBE_BURST_ATTEMPTS - 1
        );
        assert_eq!(state.failures, 1);

        drop(runtime);
        let _ = local_credential;
        cleanup_state(&state_path);
    }

    #[test]
    fn probe_interval_limits_repeated_direct_probes() {
        let mut state = PeerPathState::default();

        assert!(peer_path_probe_due(&state, SystemTime::now()));
        state.last_probe_at = Some(SystemTime::now());

        assert!(!peer_path_probe_due(&state, SystemTime::now()));
    }

    #[test]
    fn fast_probe_window_uses_short_interval_for_unknown_paths() {
        let mut state = PeerPathState {
            last_probe_at: Some(SystemTime::now() - DIRECT_FAST_PROBE_INTERVAL),
            ..PeerPathState::default()
        };

        assert!(peer_path_probe_due(&state, SystemTime::now()));

        state.fast_probe_attempts_remaining = 0;
        assert!(!peer_path_probe_due(&state, SystemTime::now()));
    }

    #[test]
    fn direct_paths_do_not_use_fast_probe_window() {
        let state = PeerPathState {
            current_path: PeerPathKind::Direct,
            last_probe_at: Some(SystemTime::now() - DIRECT_FAST_PROBE_INTERVAL),
            fast_probe_attempts_remaining: DIRECT_FAST_PROBE_BURST_ATTEMPTS,
            ..PeerPathState::default()
        };

        assert!(!peer_path_probe_due(&state, SystemTime::now()));
    }

    #[test]
    fn stale_direct_path_downgrades_to_unknown() {
        let device_id = "dev_peer".to_string();
        let mut runtime = RuntimeState::default();
        runtime.peer_path_state_by_device.insert(
            device_id.clone(),
            PeerPathState {
                current_path: PeerPathKind::Direct,
                active_endpoint: Some(endpoint(Ipv4Addr::LOCALHOST, 51820)),
                last_success_at: Some(SystemTime::now() - DIRECT_PROBE_STALE_AFTER),
                failures: DIRECT_PROBE_FAILURE_THRESHOLD - 1,
                ..PeerPathState::default()
            },
        );

        runtime.expire_stale_direct_paths(SystemTime::now());

        let state = runtime
            .peer_path_state_by_device
            .get(&device_id)
            .expect("peer state");
        assert_eq!(state.current_path, PeerPathKind::Unknown);
        assert_eq!(state.active_endpoint, None);
        assert_eq!(state.rtt_ms, None);
    }

    #[test]
    fn stale_direct_path_falls_back_to_relay_when_available() {
        let device_id = "dev_peer".to_string();
        let mut runtime = RuntimeState {
            relays: vec![RelayConfig {
                relay_id: "relay_1".to_string(),
                url: "tcp://127.0.0.1:9443".to_string(),
                region: "local".to_string(),
            }],
            ..RuntimeState::default()
        };
        runtime.peer_path_state_by_device.insert(
            device_id.clone(),
            PeerPathState {
                current_path: PeerPathKind::Direct,
                active_endpoint: Some(endpoint(Ipv4Addr::LOCALHOST, 51820)),
                last_success_at: Some(SystemTime::now() - DIRECT_PROBE_STALE_AFTER),
                failures: DIRECT_PROBE_FAILURE_THRESHOLD - 1,
                ..PeerPathState::default()
            },
        );

        runtime.expire_stale_direct_paths(SystemTime::now());

        let state = runtime
            .peer_path_state_by_device
            .get(&device_id)
            .expect("peer state");
        assert_eq!(state.current_path, PeerPathKind::Relay);
        assert_eq!(
            state.active_endpoint.as_ref().expect("relay endpoint").kind,
            EndpointKind::Relay
        );
    }

    #[test]
    fn computes_probe_response_rtt() {
        let response = encode_probe_response_payload(b"probe 1000");

        assert_eq!(response, b"probe_response 1000");
        assert_eq!(probe_response_rtt_ms(&response, 1042), Some(42));
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

    fn test_peer_with_endpoint(
        device_id: &str,
        overlay_ip: Ipv4Addr,
        endpoint: Endpoint,
    ) -> PeerConfig {
        PeerConfig {
            endpoints: vec![endpoint],
            ..test_peer(device_id, overlay_ip)
        }
    }

    fn endpoint(address: Ipv4Addr, port: u16) -> Endpoint {
        Endpoint {
            kind: EndpointKind::Lan,
            address: IpAddr::from(address),
            port,
            protocol: TransportProtocol::Udp,
        }
    }

    fn receive_with_retry(runtime: &mut TransportRuntime, buffer: &mut [u8]) -> Option<usize> {
        for _ in 0..10 {
            if let Some((received, _)) = runtime.receive_frame(buffer).expect("receive frame") {
                return Some(received);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    fn receive_backend_packet_with_retry(
        backend: &CustomBackend,
        buffer: &mut [u8],
    ) -> Option<Vec<u8>> {
        for _ in 0..10 {
            if let Some(packet) = backend
                .receive_overlay_packet(buffer)
                .expect("receive overlay packet")
            {
                return Some(packet);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    fn receive_backend_control_frame_with_retry(
        backend: &CustomBackend,
        buffer: &mut [u8],
    ) -> Option<()> {
        for _ in 0..10 {
            match backend.receive_overlay_packet(buffer) {
                Ok(Some(_packet)) => return None,
                Ok(None) => return Some(()),
                Err(_) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    #[derive(Clone, Debug)]
    struct SentRelayFrame {
        source_device_id: String,
        destination_device_id: String,
        payload: String,
    }

    struct TestRelayFrameSender {
        sent: Arc<Mutex<Vec<SentRelayFrame>>>,
    }

    impl RelayFrameSender for TestRelayFrameSender {
        fn send_frame(&self, _destination_device_id: &str, message: RelayMessage) -> Result<usize> {
            let RelayMessage::Frame {
                source_device_id,
                destination_device_id,
                payload,
            } = message
            else {
                return Err(BackendError::CommandFailed(
                    "expected relay frame message".to_string(),
                ));
            };
            let size = payload.len();
            self.sent.lock().expect("sent frames").push(SentRelayFrame {
                source_device_id,
                destination_device_id,
                payload,
            });
            Ok(size)
        }
    }

    fn ipv4_packet(source: Ipv4Addr, destination: Ipv4Addr) -> Vec<u8> {
        let mut packet = vec![0u8; 20];
        packet[0] = 0x45;
        packet[8] = 64;
        packet[9] = 1;
        packet[12..16].copy_from_slice(&source.octets());
        packet[16..20].copy_from_slice(&destination.octets());
        packet
    }
}
