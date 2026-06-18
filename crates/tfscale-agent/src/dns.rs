use anyhow::{Result, bail};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, RwLock},
};
use tfscale_core::protocol::DnsRecord;
use tokio::{net::UdpSocket, task::JoinHandle};
use tracing::{info, warn};

const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;
const RCODE_NXDOMAIN: u16 = 3;
const RCODE_NOTIMP: u16 = 4;
const RESPONSE_TTL_SECONDS: u32 = 30;
const MAX_DNS_PACKET_SIZE: usize = 512;

#[derive(Clone, Debug)]
pub struct DnsConfig {
    pub listen: SocketAddr,
    pub suffix: String,
}

#[derive(Clone, Debug)]
pub struct DnsRuntime {
    records: Arc<RwLock<Vec<DnsRecord>>>,
}

impl DnsRuntime {
    pub fn new(records: Vec<DnsRecord>) -> Self {
        Self {
            records: Arc::new(RwLock::new(records)),
        }
    }

    pub fn set_records(&self, records: Vec<DnsRecord>) {
        *self.records.write().expect("DNS records lock") = records;
    }

    #[cfg(test)]
    pub fn records_len(&self) -> usize {
        self.records.read().expect("DNS records lock").len()
    }
}

pub async fn spawn_dns_proxy(config: DnsConfig, runtime: DnsRuntime) -> Result<JoinHandle<()>> {
    let socket = UdpSocket::bind(config.listen).await?;
    let listen = socket.local_addr()?;
    info!(%listen, "DNS proxy listening");

    Ok(tokio::spawn(async move {
        if let Err(error) = run_dns_proxy(socket, config, runtime).await {
            warn!(%error, "DNS proxy stopped");
        }
    }))
}

async fn run_dns_proxy(socket: UdpSocket, config: DnsConfig, runtime: DnsRuntime) -> Result<()> {
    let mut buffer = [0u8; MAX_DNS_PACKET_SIZE];
    loop {
        let (received, peer) = socket.recv_from(&mut buffer).await?;
        let response = {
            let records = runtime.records.read().expect("DNS records lock");
            build_response(&buffer[..received], &records, &config.suffix)
        };
        match response {
            Ok(bytes) => {
                socket.send_to(&bytes, peer).await?;
            }
            Err(error) => {
                warn!(%error, %peer, "ignored invalid DNS request");
            }
        }
    }
}

pub fn resolve_a(records: &[DnsRecord], qname: &str, suffix: &str) -> Option<Ipv4Addr> {
    let query_name = normalize_name(qname);
    if !query_name.ends_with(&format!(".{}", normalize_suffix(suffix))) {
        return None;
    }

    records
        .iter()
        .find(|record| {
            record.record_type.eq_ignore_ascii_case("A")
                && normalize_name(&record.name) == query_name
        })
        .and_then(|record| record.value.parse().ok())
}

fn build_response(request: &[u8], records: &[DnsRecord], suffix: &str) -> Result<Vec<u8>> {
    let query = parse_query(request)?;
    let answer = if query.qtype == TYPE_A && query.qclass == CLASS_IN {
        resolve_a(records, &query.qname, suffix)
    } else {
        None
    };
    let rcode = if query.qtype == TYPE_A {
        if answer.is_some() { 0 } else { RCODE_NXDOMAIN }
    } else {
        RCODE_NOTIMP
    };

    let mut response = Vec::with_capacity(MAX_DNS_PACKET_SIZE);
    response.extend_from_slice(&query.id.to_be_bytes());
    response.extend_from_slice(&(0x8000 | 0x0400 | 0x0080 | rcode).to_be_bytes());
    response.extend_from_slice(&1u16.to_be_bytes());
    response.extend_from_slice(&(u16::from(answer.is_some())).to_be_bytes());
    response.extend_from_slice(&0u16.to_be_bytes());
    response.extend_from_slice(&0u16.to_be_bytes());
    response.extend_from_slice(&request[12..query.question_end]);

    if let Some(ipv4) = answer {
        response.extend_from_slice(&0xC00Cu16.to_be_bytes());
        response.extend_from_slice(&TYPE_A.to_be_bytes());
        response.extend_from_slice(&CLASS_IN.to_be_bytes());
        response.extend_from_slice(&RESPONSE_TTL_SECONDS.to_be_bytes());
        response.extend_from_slice(&4u16.to_be_bytes());
        response.extend_from_slice(&ipv4.octets());
    }

    Ok(response)
}

#[derive(Debug)]
struct DnsQuery {
    id: u16,
    qname: String,
    qtype: u16,
    qclass: u16,
    question_end: usize,
}

fn parse_query(packet: &[u8]) -> Result<DnsQuery> {
    if packet.len() < 12 {
        bail!("DNS packet is too short");
    }

    let qdcount = u16::from_be_bytes([packet[4], packet[5]]);
    if qdcount != 1 {
        bail!("only one DNS question is supported");
    }

    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let mut offset = 12;
    let mut labels = Vec::new();
    loop {
        if offset >= packet.len() {
            bail!("DNS question name is truncated");
        }
        let len = packet[offset] as usize;
        offset += 1;
        if len == 0 {
            break;
        }
        if len & 0xC0 != 0 {
            bail!("compressed DNS question names are not supported");
        }
        if len > 63 || offset + len > packet.len() {
            bail!("invalid DNS label length");
        }
        labels.push(std::str::from_utf8(&packet[offset..offset + len])?.to_string());
        offset += len;
    }

    if offset + 4 > packet.len() {
        bail!("DNS question type/class is truncated");
    }

    let qtype = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
    let qclass = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]);
    let question_end = offset + 4;

    Ok(DnsQuery {
        id,
        qname: labels.join("."),
        qtype,
        qclass,
        question_end,
    })
}

fn normalize_name(value: &str) -> String {
    value.trim_end_matches('.').to_ascii_lowercase()
}

fn normalize_suffix(value: &str) -> String {
    value.trim_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_mesh_a_record() {
        let records = vec![record("devbox.mesh", "100.64.0.2")];

        assert_eq!(
            resolve_a(&records, "devbox.mesh", "mesh"),
            Some(Ipv4Addr::new(100, 64, 0, 2))
        );
    }

    #[test]
    fn resolves_case_insensitive_and_trailing_dot() {
        let records = vec![record("devbox.mesh", "100.64.0.2")];

        assert_eq!(
            resolve_a(&records, "DevBox.Mesh.", "mesh"),
            Some(Ipv4Addr::new(100, 64, 0, 2))
        );
    }

    #[test]
    fn ignores_non_mesh_query() {
        let records = vec![record("devbox.mesh", "100.64.0.2")];

        assert_eq!(resolve_a(&records, "example.com", "mesh"), None);
    }

    #[test]
    fn returns_none_for_missing_record() {
        let records = vec![record("devbox.mesh", "100.64.0.2")];

        assert_eq!(resolve_a(&records, "missing.mesh", "mesh"), None);
    }

    #[test]
    fn builds_a_response() {
        let request = query_packet(0x1234, "devbox.mesh", TYPE_A);
        let response = build_response(&request, &[record("devbox.mesh", "100.64.0.2")], "mesh")
            .expect("DNS response");

        assert_eq!(&response[0..2], &0x1234u16.to_be_bytes());
        assert_eq!(u16::from_be_bytes([response[6], response[7]]), 1);
        assert_eq!(&response[response.len() - 4..], &[100, 64, 0, 2]);
    }

    #[test]
    fn builds_nxdomain_response() {
        let request = query_packet(0x1234, "missing.mesh", TYPE_A);
        let response = build_response(&request, &[record("devbox.mesh", "100.64.0.2")], "mesh")
            .expect("DNS response");
        let flags = u16::from_be_bytes([response[2], response[3]]);

        assert_eq!(flags & 0x000F, RCODE_NXDOMAIN);
        assert_eq!(u16::from_be_bytes([response[6], response[7]]), 0);
    }

    #[tokio::test]
    async fn udp_proxy_answers_a_query() {
        let runtime = DnsRuntime::new(vec![record("devbox.mesh", "100.64.0.2")]);
        let server_socket = UdpSocket::bind("127.0.0.1:0").await.expect("server socket");
        let server_addr = server_socket.local_addr().expect("server addr");
        let handle = tokio::spawn(run_dns_proxy(
            server_socket,
            DnsConfig {
                listen: server_addr,
                suffix: "mesh".to_string(),
            },
            runtime,
        ));
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("client socket");
        let request = query_packet(0x4321, "devbox.mesh", TYPE_A);
        let mut response = [0u8; MAX_DNS_PACKET_SIZE];

        socket
            .send_to(&request, server_addr)
            .await
            .expect("send DNS query");
        let (received, _peer) = socket.recv_from(&mut response).await.expect("DNS response");
        handle.abort();

        assert_eq!(&response[0..2], &0x4321u16.to_be_bytes());
        assert_eq!(u16::from_be_bytes([response[6], response[7]]), 1);
        assert_eq!(&response[received - 4..received], &[100, 64, 0, 2]);
    }

    fn record(name: &str, value: &str) -> DnsRecord {
        DnsRecord {
            device_id: "dev_test".to_string(),
            name: name.to_string(),
            record_type: "A".to_string(),
            value: value.to_string(),
        }
    }

    fn query_packet(id: u16, name: &str, qtype: u16) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.extend_from_slice(&id.to_be_bytes());
        packet.extend_from_slice(&0x0100u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());
        for label in name.split('.') {
            packet.push(label.len() as u8);
            packet.extend_from_slice(label.as_bytes());
        }
        packet.push(0);
        packet.extend_from_slice(&qtype.to_be_bytes());
        packet.extend_from_slice(&CLASS_IN.to_be_bytes());
        packet
    }
}
