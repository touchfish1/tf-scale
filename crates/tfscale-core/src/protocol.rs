use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateAuthKeyResponse {
    pub id: String,
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub id: String,
    pub hostname: String,
    pub ipv4: String,
    pub os: String,
    pub arch: String,
    pub backend_type: String,
    pub last_seen_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameDeviceRequest {
    pub hostname: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterDeviceRequest {
    pub auth_key: String,
    pub hostname: String,
    pub machine_key: String,
    pub backend_type: String,
    pub backend_public_credential: String,
    pub os: String,
    pub arch: String,
    pub client_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterDeviceResponse {
    pub device_id: String,
    pub node_key: String,
    pub ipv4: String,
    pub network_id: String,
    pub poll_interval_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointPayload {
    pub kind: String,
    pub address: String,
    pub port: u16,
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackendStatusPayload {
    pub backend_type: String,
    pub interface: String,
    pub healthy: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub device_id: String,
    pub node_key: String,
    pub endpoints: Vec<EndpointPayload>,
    pub backend_status: BackendStatusPayload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub ok: bool,
    pub network_map_version: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointProbeRequest {
    pub device_id: String,
    pub node_key: String,
    pub protocol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointProbeResponse {
    pub observed_address: String,
    pub observed_port: u16,
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_probe_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_probe_port: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkMapResponse {
    pub network_map_version: i64,
    pub self_device: NetworkMapSelf,
    pub peers: Vec<NetworkMapPeer>,
    #[serde(default)]
    pub relays: Vec<RelayMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkMapSelf {
    pub device_id: String,
    pub hostname: String,
    pub ipv4: String,
    pub backend_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkMapPeer {
    pub device_id: String,
    pub hostname: String,
    pub ipv4: String,
    pub backend_type: String,
    pub backend_public_credential: String,
    pub endpoints: Vec<EndpointPayload>,
    pub allowed_routes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayMetadata {
    pub relay_id: String,
    pub url: String,
    pub region: String,
    pub healthy: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayMessage {
    Register {
        device_id: String,
        node_key: String,
    },
    Frame {
        source_device_id: String,
        destination_device_id: String,
        payload: String,
    },
    Error {
        message: String,
    },
}
