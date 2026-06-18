#![allow(dead_code)]

use crate::{
    PUBLIC_CREDENTIAL_PREFIX, decode_base64_key,
    frame::{EncodedFrame, FrameDeviceId, FrameHeader},
    nonce::{ReplayWindow, SendNonceState},
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use sha2::Sha256;
use tfscale_net::{BackendError, Result};
use x25519_dalek::{PublicKey, StaticSecret};

const SESSION_SALT: &[u8] = b"tf-scale/custom/v1/session";
const SESSION_INFO_PREFIX: &[u8] = b"tf-scale/custom/v1/aead-key";
const DIRECTION_INFO_PREFIX: &[u8] = b"tf-scale/custom/v1/direction";

#[derive(Clone)]
pub(crate) struct PeerCryptoSession {
    local_id: FrameDeviceId,
    peer_id: FrameDeviceId,
    cipher: XChaCha20Poly1305,
    send_nonces: SendNonceState,
    receive_window: ReplayWindow,
    receive_direction_prefix: [u8; 8],
}

impl PeerCryptoSession {
    pub fn new(
        local_private_key: [u8; 32],
        local_public_credential: &str,
        peer_public_credential: &str,
        local_id: FrameDeviceId,
        peer_id: FrameDeviceId,
    ) -> Result<Self> {
        let shared_secret = shared_secret(local_private_key, peer_public_credential)?;
        let session_key = derive_session_key(
            &shared_secret,
            local_public_credential,
            peer_public_credential,
            local_id,
            peer_id,
        )?;
        let send_direction_prefix = derive_direction_prefix(&shared_secret, local_id, peer_id)?;
        let receive_direction_prefix = derive_direction_prefix(&shared_secret, peer_id, local_id)?;

        Ok(Self {
            local_id,
            peer_id,
            cipher: XChaCha20Poly1305::new((&session_key).into()),
            send_nonces: SendNonceState::new(send_direction_prefix)?,
            receive_window: ReplayWindow::default(),
            receive_direction_prefix,
        })
    }

    pub fn seal(&mut self, plaintext_packet: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.send_nonces.next()?;
        let header = FrameHeader::data(self.local_id, self.peer_id, nonce);
        let associated_data = header.encode()?;
        let ciphertext = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext_packet,
                    aad: &associated_data,
                },
            )
            .map_err(|_| BackendError::CommandFailed("packet encryption failed".to_string()))?;

        EncodedFrame { header, ciphertext }.encode()
    }

    pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        let encoded = EncodedFrame::decode(frame)?;
        if encoded.header.source != self.peer_id {
            return Err(BackendError::CommandFailed(
                "frame source does not match peer session".to_string(),
            ));
        }
        if encoded.header.destination != self.local_id {
            return Err(BackendError::CommandFailed(
                "frame destination does not match local session".to_string(),
            ));
        }
        if encoded.header.nonce[..8] != self.receive_direction_prefix {
            return Err(BackendError::CommandFailed(
                "frame nonce direction does not match peer session".to_string(),
            ));
        }

        let associated_data = encoded.header.encode()?;
        let plaintext = self
            .cipher
            .decrypt(
                XNonce::from_slice(&encoded.header.nonce),
                Payload {
                    msg: &encoded.ciphertext,
                    aad: &associated_data,
                },
            )
            .map_err(|_| BackendError::CommandFailed("packet authentication failed".to_string()))?;
        self.receive_window.accept(&encoded.header.nonce)?;
        Ok(plaintext)
    }
}

pub(crate) fn decode_public_credential(value: &str) -> Result<[u8; 32]> {
    let encoded = value
        .strip_prefix(PUBLIC_CREDENTIAL_PREFIX)
        .ok_or_else(|| {
            BackendError::CommandFailed("unsupported peer credential prefix".to_string())
        })?;
    let bytes = decode_base64_key(encoded)?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        BackendError::CommandFailed(format!(
            "peer credential has invalid key length: {}",
            bytes.len()
        ))
    })
}

fn shared_secret(local_private_key: [u8; 32], peer_public_credential: &str) -> Result<[u8; 32]> {
    let private = StaticSecret::from(local_private_key);
    let peer_public = PublicKey::from(decode_public_credential(peer_public_credential)?);
    Ok(private.diffie_hellman(&peer_public).to_bytes())
}

fn derive_session_key(
    shared_secret: &[u8; 32],
    local_public_credential: &str,
    peer_public_credential: &str,
    local_id: FrameDeviceId,
    peer_id: FrameDeviceId,
) -> Result<[u8; 32]> {
    let mut info = Vec::new();
    info.extend_from_slice(SESSION_INFO_PREFIX);
    append_sorted_pair(&mut info, local_id.as_bytes(), peer_id.as_bytes());
    append_sorted_pair(
        &mut info,
        local_public_credential.as_bytes(),
        peer_public_credential.as_bytes(),
    );

    hkdf_expand(shared_secret, SESSION_SALT, &info)
}

fn derive_direction_prefix(
    shared_secret: &[u8; 32],
    source: FrameDeviceId,
    destination: FrameDeviceId,
) -> Result<[u8; 8]> {
    let mut info = Vec::new();
    info.extend_from_slice(DIRECTION_INFO_PREFIX);
    info.extend_from_slice(source.as_bytes());
    info.extend_from_slice(destination.as_bytes());

    hkdf_expand(shared_secret, SESSION_SALT, &info)
}

fn hkdf_expand<const N: usize>(secret: &[u8; 32], salt: &[u8], info: &[u8]) -> Result<[u8; N]> {
    let hkdf = Hkdf::<Sha256>::new(Some(salt), secret);
    let mut output = [0; N];
    hkdf.expand(info, &mut output)
        .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
    Ok(output)
}

fn append_sorted_pair(output: &mut Vec<u8>, left: &[u8], right: &[u8]) {
    if left <= right {
        output.extend_from_slice(left);
        output.extend_from_slice(right);
    } else {
        output.extend_from_slice(right);
        output.extend_from_slice(left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::NONCE_LEN;
    use crate::{decode_key, encode_public_credential, generate_identity};

    fn session_pair() -> (PeerCryptoSession, PeerCryptoSession) {
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
        .expect("left session");
        let right = PeerCryptoSession::new(
            decode_key(&right_identity.private_key).expect("right private key"),
            &right_identity.public_key,
            &left_identity.public_key,
            right_id,
            left_id,
        )
        .expect("right session");

        (left, right)
    }

    #[test]
    fn decodes_public_credentials() {
        let credential = encode_public_credential([7; 32]);

        let decoded = decode_public_credential(&credential).expect("decoded credential");

        assert_eq!(decoded, [7; 32]);
    }

    #[test]
    fn rejects_invalid_public_credentials() {
        let error = decode_public_credential("peer-public-key").expect_err("invalid credential");

        assert!(
            error
                .to_string()
                .contains("unsupported peer credential prefix")
        );
    }

    #[test]
    fn seals_and_opens_packet_frame() {
        let (mut left, mut right) = session_pair();
        let packet = [0x45, 0x00, 0x00, 0x54];

        let frame = left.seal(&packet).expect("sealed frame");
        let opened = right.open(&frame).expect("opened frame");

        assert_eq!(opened, packet);
    }

    #[test]
    fn rejects_tampered_header() {
        let (mut left, mut right) = session_pair();
        let mut frame = left.seal(&[1, 2, 3, 4]).expect("sealed frame");
        frame[4] ^= 1;

        let error = right.open(&frame).expect_err("tampered header");

        assert!(
            error.to_string().contains("frame source")
                || error.to_string().contains("packet authentication failed")
        );
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let (mut left, mut right) = session_pair();
        let mut frame = left.seal(&[1, 2, 3, 4]).expect("sealed frame");
        let last = frame.len() - 1;
        frame[last] ^= 1;

        let error = right.open(&frame).expect_err("tampered ciphertext");

        assert!(error.to_string().contains("packet authentication failed"));
    }

    #[test]
    fn rejects_replayed_frame() {
        let (mut left, mut right) = session_pair();
        let frame = left.seal(&[1, 2, 3, 4]).expect("sealed frame");

        right.open(&frame).expect("first open");
        let error = right.open(&frame).expect_err("replay");

        assert!(error.to_string().contains("replayed frame nonce"));
    }

    #[test]
    fn different_peer_pairs_cannot_open_frames() {
        let (mut left, _) = session_pair();
        let (_, mut unrelated) = session_pair();
        let frame = left.seal(&[1, 2, 3, 4]).expect("sealed frame");

        let error = unrelated.open(&frame).expect_err("wrong session");

        assert!(
            error.to_string().contains("frame source")
                || error.to_string().contains("frame nonce direction")
                || error.to_string().contains("packet authentication failed")
        );
    }

    #[test]
    fn nonce_has_expected_length() {
        assert_eq!(NONCE_LEN, 24);
    }
}
