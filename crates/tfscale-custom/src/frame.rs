#![allow(dead_code)]

use tfscale_net::{BackendError, Result};
use uuid::Uuid;

pub(crate) const FRAME_VERSION: u8 = 1;
pub(crate) const FRAME_TYPE_DATA: u8 = 1;
pub(crate) const FRAME_TYPE_PROBE: u8 = 2;
pub(crate) const FRAME_TYPE_PROBE_RESPONSE: u8 = 3;
pub(crate) const HEADER_LEN: usize = 60;
pub(crate) const DEVICE_ID_LEN: usize = 16;
pub(crate) const NONCE_LEN: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct FrameDeviceId([u8; DEVICE_ID_LEN]);

impl FrameDeviceId {
    pub fn new(bytes: [u8; DEVICE_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_device_id(value: &str) -> Result<Self> {
        let uuid = value
            .strip_prefix("dev_")
            .ok_or_else(|| {
                BackendError::CommandFailed("device ID is missing dev_ prefix".to_string())
            })
            .and_then(|encoded| {
                Uuid::parse_str(encoded)
                    .map_err(|error| BackendError::CommandFailed(error.to_string()))
            })?;
        Ok(Self(*uuid.as_bytes()))
    }

    pub fn as_bytes(&self) -> &[u8; DEVICE_ID_LEN] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FrameHeader {
    pub version: u8,
    pub message_type: u8,
    pub flags: u8,
    pub source: FrameDeviceId,
    pub destination: FrameDeviceId,
    pub nonce: [u8; NONCE_LEN],
}

impl FrameHeader {
    pub fn new(
        source: FrameDeviceId,
        destination: FrameDeviceId,
        nonce: [u8; NONCE_LEN],
        message_type: u8,
    ) -> Self {
        Self {
            version: FRAME_VERSION,
            message_type,
            flags: 0,
            source,
            destination,
            nonce,
        }
    }

    pub fn data(source: FrameDeviceId, destination: FrameDeviceId, nonce: [u8; NONCE_LEN]) -> Self {
        Self::new(source, destination, nonce, FRAME_TYPE_DATA)
    }

    pub fn encode(&self) -> Result<[u8; HEADER_LEN]> {
        if self.version != FRAME_VERSION {
            return Err(BackendError::CommandFailed(format!(
                "unsupported frame version: {}",
                self.version
            )));
        }
        if !matches!(
            self.message_type,
            FRAME_TYPE_DATA | FRAME_TYPE_PROBE | FRAME_TYPE_PROBE_RESPONSE
        ) {
            return Err(BackendError::CommandFailed(format!(
                "unsupported frame message type: {}",
                self.message_type
            )));
        }
        if self.flags != 0 {
            return Err(BackendError::CommandFailed(format!(
                "unsupported frame flags: {}",
                self.flags
            )));
        }

        let mut output = [0; HEADER_LEN];
        output[0] = self.version;
        output[1] = self.message_type;
        output[2] = self.flags;
        output[3] = 0;
        output[4..20].copy_from_slice(self.source.as_bytes());
        output[20..36].copy_from_slice(self.destination.as_bytes());
        output[36..60].copy_from_slice(&self.nonce);
        Ok(output)
    }

    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < HEADER_LEN {
            return Err(BackendError::CommandFailed(format!(
                "malformed frame length: {}",
                input.len()
            )));
        }
        if input[0] != FRAME_VERSION {
            return Err(BackendError::CommandFailed(format!(
                "unsupported frame version: {}",
                input[0]
            )));
        }
        if !matches!(
            input[1],
            FRAME_TYPE_DATA | FRAME_TYPE_PROBE | FRAME_TYPE_PROBE_RESPONSE
        ) {
            return Err(BackendError::CommandFailed(format!(
                "unsupported frame message type: {}",
                input[1]
            )));
        }
        if input[2] != 0 {
            return Err(BackendError::CommandFailed(format!(
                "unsupported frame flags: {}",
                input[2]
            )));
        }
        if input[3] != 0 {
            return Err(BackendError::CommandFailed(
                "reserved frame byte must be zero".to_string(),
            ));
        }

        let mut source = [0; DEVICE_ID_LEN];
        source.copy_from_slice(&input[4..20]);
        let mut destination = [0; DEVICE_ID_LEN];
        destination.copy_from_slice(&input[20..36]);
        let mut nonce = [0; NONCE_LEN];
        nonce.copy_from_slice(&input[36..60]);

        Ok(Self {
            version: input[0],
            message_type: input[1],
            flags: input[2],
            source: FrameDeviceId::new(source),
            destination: FrameDeviceId::new(destination),
            nonce,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EncodedFrame {
    pub header: FrameHeader,
    pub ciphertext: Vec<u8>,
}

impl EncodedFrame {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let header = self.header.encode()?;
        let mut output = Vec::with_capacity(HEADER_LEN + self.ciphertext.len());
        output.extend_from_slice(&header);
        output.extend_from_slice(&self.ciphertext);
        Ok(output)
    }

    pub fn decode(input: &[u8]) -> Result<Self> {
        let header = FrameHeader::decode(input)?;
        Ok(Self {
            header,
            ciphertext: input[HEADER_LEN..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_header_round_trips() {
        let source = FrameDeviceId::new([1; DEVICE_ID_LEN]);
        let destination = FrameDeviceId::new([2; DEVICE_ID_LEN]);
        let header = FrameHeader::data(source, destination, [3; NONCE_LEN]);

        let encoded = header.encode().expect("encoded header");
        let decoded = FrameHeader::decode(&encoded).expect("decoded header");

        assert_eq!(decoded, header);
    }

    #[test]
    fn probe_frame_header_round_trips() {
        let source = FrameDeviceId::new([1; DEVICE_ID_LEN]);
        let destination = FrameDeviceId::new([2; DEVICE_ID_LEN]);
        let header = FrameHeader::new(source, destination, [3; NONCE_LEN], FRAME_TYPE_PROBE);

        let encoded = header.encode().expect("encoded header");
        let decoded = FrameHeader::decode(&encoded).expect("decoded header");

        assert_eq!(decoded, header);
    }

    #[test]
    fn rejects_short_frames() {
        let error = FrameHeader::decode(&[0; HEADER_LEN - 1]).expect_err("short frame");

        assert!(error.to_string().contains("malformed frame length"));
    }

    #[test]
    fn rejects_unknown_frame_version() {
        let mut input = [0; HEADER_LEN];
        input[0] = 2;
        input[1] = FRAME_TYPE_DATA;

        let error = FrameHeader::decode(&input).expect_err("unknown version");

        assert!(error.to_string().contains("unsupported frame version"));
    }

    #[test]
    fn parses_generated_device_id() {
        let id = format!("dev_{}", Uuid::now_v7().simple());

        FrameDeviceId::from_device_id(&id).expect("frame device id");
    }
}
