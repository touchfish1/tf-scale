use async_trait::async_trait;
use tfscale_net::{
    BackendCapabilities, BackendCredential, BackendStatus, BackendType, LocalBackendConfig,
    NetworkBackend, PeerConfig, Result,
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct CustomBackend {
    interface_name: String,
}

impl CustomBackend {
    pub fn new(interface_name: impl Into<String>) -> Self {
        Self {
            interface_name: interface_name.into(),
        }
    }
}

impl Default for CustomBackend {
    fn default() -> Self {
        Self::new("tfscale0")
    }
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
        Ok(BackendCredential {
            public: format!("tfpk_{}", Uuid::now_v7().simple()),
        })
    }

    async fn apply_local_config(&self, _config: LocalBackendConfig) -> Result<()> {
        Ok(())
    }

    async fn apply_peer_map(&self, _peers: Vec<PeerConfig>) -> Result<()> {
        Ok(())
    }

    async fn status(&self) -> Result<BackendStatus> {
        Ok(BackendStatus {
            backend_type: self.backend_type(),
            interface_name: self.interface_name.clone(),
            healthy: false,
            message: Some("custom userspace backend skeleton is not implemented yet".to_string()),
        })
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_tfscale_interface_name() {
        let backend = CustomBackend::default();

        assert_eq!(backend.interface_name, "tfscale0");
    }
}
