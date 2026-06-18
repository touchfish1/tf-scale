#![allow(dead_code)]

use std::net::Ipv4Addr;
use tfscale_net::{BackendError, Result};

const IPV4_MIN_HEADER_LEN: usize = 20;
const IPV4_VERSION: u8 = 4;
const IPV4_DESTINATION_OFFSET: usize = 16;

pub(crate) fn ipv4_destination(packet: &[u8]) -> Result<Ipv4Addr> {
    if packet.len() < IPV4_MIN_HEADER_LEN {
        return Err(BackendError::CommandFailed(format!(
            "packet is too short for IPv4 header: {}",
            packet.len()
        )));
    }

    let version = packet[0] >> 4;
    if version != IPV4_VERSION {
        return Err(BackendError::CommandFailed(format!(
            "unsupported packet IP version: {version}"
        )));
    }

    let header_len = usize::from(packet[0] & 0x0f) * 4;
    if header_len < IPV4_MIN_HEADER_LEN || packet.len() < header_len {
        return Err(BackendError::CommandFailed(
            "invalid IPv4 header length".to_string(),
        ));
    }

    Ok(Ipv4Addr::new(
        packet[IPV4_DESTINATION_OFFSET],
        packet[IPV4_DESTINATION_OFFSET + 1],
        packet[IPV4_DESTINATION_OFFSET + 2],
        packet[IPV4_DESTINATION_OFFSET + 3],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipv4_destination() {
        let mut packet = [0u8; 20];
        packet[0] = 0x45;
        packet[16..20].copy_from_slice(&[100, 64, 0, 3]);

        assert_eq!(
            ipv4_destination(&packet).expect("destination"),
            Ipv4Addr::new(100, 64, 0, 3)
        );
    }

    #[test]
    fn rejects_non_ipv4_packets() {
        let mut packet = [0u8; 20];
        packet[0] = 0x60;

        let error = ipv4_destination(&packet).expect_err("non-ipv4");

        assert!(error.to_string().contains("unsupported packet IP version"));
    }

    #[test]
    fn rejects_short_packets() {
        let error = ipv4_destination(&[0u8; 19]).expect_err("short packet");

        assert!(error.to_string().contains("packet is too short"));
    }
}
