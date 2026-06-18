#![allow(dead_code)]

use crate::crypto::PeerCryptoSession;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::SystemTime,
};
use tfscale_net::{BackendError, Result};
use tfscale_net::{Endpoint, EndpointKind, TransportProtocol};

#[derive(Debug)]
pub(crate) struct TransportRuntime {
    socket: UdpSocket,
    local_endpoint: Endpoint,
    status: TransportStatus,
}

impl TransportRuntime {
    pub fn bind(listen_port: u16) -> Result<Self> {
        let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, listen_port));
        let socket = UdpSocket::bind(bind_addr)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        socket
            .set_nonblocking(true)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        let local_addr = socket
            .local_addr()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        let local_endpoint = Endpoint {
            kind: EndpointKind::Lan,
            address: IpAddr::V4(discover_lan_ipv4().unwrap_or(Ipv4Addr::LOCALHOST)),
            port: local_addr.port(),
            protocol: TransportProtocol::Udp,
        };

        Ok(Self {
            socket,
            local_endpoint,
            status: TransportStatus {
                udp_bound: true,
                local_endpoints: 1,
                bound_at: Some(SystemTime::now()),
                ..TransportStatus::default()
            },
        })
    }

    pub fn local_endpoints(&self) -> Vec<Endpoint> {
        vec![self.local_endpoint.clone()]
    }

    pub fn status(&self) -> TransportStatus {
        self.status.clone()
    }

    pub fn send_frame(&mut self, endpoint: &Endpoint, frame: &[u8]) -> Result<usize> {
        let socket_addr = endpoint_socket_addr(endpoint)?;
        match self.socket.send_to(frame, socket_addr) {
            Ok(bytes) => {
                self.status.tx_packets += 1;
                Ok(bytes)
            }
            Err(error) => {
                self.status.tx_drops += 1;
                Err(BackendError::CommandFailed(error.to_string()))
            }
        }
    }

    pub fn send_packet(
        &mut self,
        endpoint: &Endpoint,
        crypto: &mut PeerCryptoSession,
        packet: &[u8],
    ) -> Result<usize> {
        let frame = match crypto.seal(packet) {
            Ok(frame) => frame,
            Err(error) => {
                self.status.tx_drops += 1;
                return Err(error);
            }
        };
        self.send_frame(endpoint, &frame)
    }

    pub fn receive_frame(&mut self, buffer: &mut [u8]) -> Result<Option<(usize, SocketAddr)>> {
        match self.socket.recv_from(buffer) {
            Ok(received) => {
                self.status.rx_packets += 1;
                Ok(Some(received))
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => {
                self.status.rx_drops += 1;
                Err(BackendError::CommandFailed(error.to_string()))
            }
        }
    }

    pub fn receive_packet(
        &mut self,
        crypto: &mut PeerCryptoSession,
        buffer: &mut [u8],
    ) -> Result<Option<Vec<u8>>> {
        let Some((received, _)) = self.receive_frame(buffer)? else {
            return Ok(None);
        };
        match crypto.open(&buffer[..received]) {
            Ok(packet) => Ok(Some(packet)),
            Err(error) => {
                self.status.rx_drops += 1;
                Err(error)
            }
        }
    }

    pub fn record_rx_drop(&mut self) {
        self.status.rx_drops += 1;
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.socket
            .local_addr()
            .map_err(|error| BackendError::CommandFailed(error.to_string()))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TransportStatus {
    pub udp_bound: bool,
    pub local_endpoints: usize,
    pub transport_peers: usize,
    pub reachable_peers: usize,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_drops: u64,
    pub rx_drops: u64,
    pub bound_at: Option<SystemTime>,
}

pub(crate) fn select_udp_endpoint(endpoints: &[Endpoint]) -> Option<Endpoint> {
    endpoints
        .iter()
        .find(|endpoint| {
            endpoint.kind == EndpointKind::Lan && endpoint.protocol == TransportProtocol::Udp
        })
        .cloned()
}

fn endpoint_socket_addr(endpoint: &Endpoint) -> Result<SocketAddr> {
    if endpoint.protocol != TransportProtocol::Udp {
        return Err(BackendError::CommandFailed(
            "endpoint is not UDP transport".to_string(),
        ));
    }

    Ok(SocketAddr::new(endpoint.address, endpoint.port))
}

fn discover_lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
    socket
        .connect(SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), 9)))
        .ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) if !address.is_unspecified() => Some(address),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crypto::PeerCryptoSession, decode_key, frame::FrameDeviceId, generate_identity};

    #[test]
    fn selects_lan_udp_endpoint() {
        let endpoint = select_udp_endpoint(&[
            Endpoint {
                kind: EndpointKind::Public,
                address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
                port: 51820,
                protocol: TransportProtocol::Udp,
            },
            Endpoint {
                kind: EndpointKind::Lan,
                address: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 30)),
                port: 51820,
                protocol: TransportProtocol::Udp,
            },
        ])
        .expect("selected endpoint");

        assert_eq!(endpoint.kind, EndpointKind::Lan);
        assert_eq!(endpoint.protocol, TransportProtocol::Udp);
    }

    #[test]
    fn ignores_non_udp_endpoints() {
        let endpoint = select_udp_endpoint(&[Endpoint {
            kind: EndpointKind::Lan,
            address: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 30)),
            port: 443,
            protocol: TransportProtocol::Tcp,
        }]);

        assert!(endpoint.is_none());
    }

    #[test]
    fn binds_udp_socket_and_reports_endpoint() {
        let runtime = TransportRuntime::bind(0).expect("transport runtime");
        let endpoint = runtime
            .local_endpoints()
            .into_iter()
            .next()
            .expect("local endpoint");

        assert_eq!(endpoint.kind, EndpointKind::Lan);
        assert_eq!(endpoint.protocol, TransportProtocol::Udp);
        assert_eq!(
            endpoint.port,
            runtime.local_addr().expect("local addr").port()
        );
        assert!(runtime.status().udp_bound);
    }

    #[test]
    fn sends_and_receives_udp_frame_on_loopback() {
        let mut left = TransportRuntime::bind(0).expect("left runtime");
        let mut right = TransportRuntime::bind(0).expect("right runtime");
        let right_endpoint = Endpoint {
            kind: EndpointKind::Lan,
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: right.local_addr().expect("right addr").port(),
            protocol: TransportProtocol::Udp,
        };

        left.send_frame(&right_endpoint, b"tfscale-frame")
            .expect("send frame");

        let mut buffer = [0u8; 64];
        let received = receive_with_retry(&mut right, &mut buffer).expect("received frame");

        assert_eq!(&buffer[..received], b"tfscale-frame");
        assert_eq!(left.status().tx_packets, 1);
        assert_eq!(right.status().rx_packets, 1);
    }

    #[test]
    fn sends_and_receives_encrypted_packet_on_loopback() {
        let mut left = TransportRuntime::bind(0).expect("left runtime");
        let mut right = TransportRuntime::bind(0).expect("right runtime");
        let right_endpoint = Endpoint {
            kind: EndpointKind::Lan,
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: right.local_addr().expect("right addr").port(),
            protocol: TransportProtocol::Udp,
        };
        let (mut left_crypto, mut right_crypto) = crypto_pair();
        let packet = ipv4_packet(Ipv4Addr::new(100, 64, 0, 2), Ipv4Addr::new(100, 64, 0, 3));

        left.send_packet(&right_endpoint, &mut left_crypto, &packet)
            .expect("send encrypted packet");

        let mut buffer = [0u8; 1500];
        let opened =
            receive_packet_with_retry(&mut right, &mut right_crypto, &mut buffer).expect("packet");

        assert_eq!(opened, packet);
        assert_eq!(left.status().tx_packets, 1);
        assert_eq!(right.status().rx_packets, 1);
        assert_eq!(right.status().rx_drops, 0);
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

    fn receive_packet_with_retry(
        runtime: &mut TransportRuntime,
        crypto: &mut PeerCryptoSession,
        buffer: &mut [u8],
    ) -> Option<Vec<u8>> {
        for _ in 0..10 {
            if let Some(packet) = runtime
                .receive_packet(crypto, buffer)
                .expect("receive packet")
            {
                return Some(packet);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    fn crypto_pair() -> (PeerCryptoSession, PeerCryptoSession) {
        let left_identity = generate_identity();
        let right_identity = generate_identity();
        let left_id = FrameDeviceId::new([1; 16]);
        let right_id = FrameDeviceId::new([2; 16]);

        let left = PeerCryptoSession::new(
            decode_key(&left_identity.private_key).expect("left private key"),
            &left_identity.public_key,
            &right_identity.public_key,
            left_id,
            right_id,
        )
        .expect("left crypto");
        let right = PeerCryptoSession::new(
            decode_key(&right_identity.private_key).expect("right private key"),
            &right_identity.public_key,
            &left_identity.public_key,
            right_id,
            left_id,
        )
        .expect("right crypto");

        (left, right)
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
