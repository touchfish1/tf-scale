#![allow(dead_code)]

use crate::frame::NONCE_LEN;
use tfscale_net::{BackendError, Result};

const DIRECTION_PREFIX_LEN: usize = 8;
const SESSION_SALT_LEN: usize = 8;
const COUNTER_LEN: usize = 8;
const REPLAY_WINDOW_BITS: u64 = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SendNonceState {
    direction_prefix: [u8; DIRECTION_PREFIX_LEN],
    session_salt: [u8; SESSION_SALT_LEN],
    counter: u64,
}

impl SendNonceState {
    pub fn new(direction_prefix: [u8; DIRECTION_PREFIX_LEN]) -> Result<Self> {
        let mut session_salt = [0; SESSION_SALT_LEN];
        getrandom::fill(&mut session_salt)
            .map_err(|error| BackendError::CommandFailed(error.to_string()))?;
        Ok(Self {
            direction_prefix,
            session_salt,
            counter: 0,
        })
    }

    pub fn with_salt(
        direction_prefix: [u8; DIRECTION_PREFIX_LEN],
        session_salt: [u8; SESSION_SALT_LEN],
    ) -> Self {
        Self {
            direction_prefix,
            session_salt,
            counter: 0,
        }
    }

    pub fn next(&mut self) -> Result<[u8; NONCE_LEN]> {
        if self.counter == u64::MAX {
            return Err(BackendError::CommandFailed(
                "send nonce counter exhausted".to_string(),
            ));
        }

        let mut nonce = [0; NONCE_LEN];
        nonce[..DIRECTION_PREFIX_LEN].copy_from_slice(&self.direction_prefix);
        nonce[DIRECTION_PREFIX_LEN..DIRECTION_PREFIX_LEN + SESSION_SALT_LEN]
            .copy_from_slice(&self.session_salt);
        nonce[DIRECTION_PREFIX_LEN + SESSION_SALT_LEN..]
            .copy_from_slice(&self.counter.to_be_bytes());
        self.counter += 1;
        Ok(nonce)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReplayWindow {
    highest: Option<u64>,
    seen: u64,
}

impl ReplayWindow {
    pub fn accept(&mut self, nonce: &[u8; NONCE_LEN]) -> Result<()> {
        let counter = nonce_counter(nonce);
        match self.highest {
            None => {
                self.highest = Some(counter);
                self.seen = 1;
                Ok(())
            }
            Some(highest) if counter > highest => {
                let shift = counter - highest;
                if shift >= REPLAY_WINDOW_BITS {
                    self.seen = 1;
                } else {
                    self.seen = (self.seen << shift) | 1;
                }
                self.highest = Some(counter);
                Ok(())
            }
            Some(highest) => {
                let behind = highest - counter;
                if behind >= REPLAY_WINDOW_BITS {
                    return Err(BackendError::CommandFailed("stale frame nonce".to_string()));
                }

                let bit = 1u64 << behind;
                if self.seen & bit != 0 {
                    return Err(BackendError::CommandFailed(
                        "replayed frame nonce".to_string(),
                    ));
                }

                self.seen |= bit;
                Ok(())
            }
        }
    }
}

pub(crate) fn nonce_counter(nonce: &[u8; NONCE_LEN]) -> u64 {
    let mut bytes = [0; COUNTER_LEN];
    bytes.copy_from_slice(&nonce[DIRECTION_PREFIX_LEN + SESSION_SALT_LEN..]);
    u64::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_nonce_counter_increments() {
        let mut state = SendNonceState::with_salt([1; 8], [2; 8]);

        let first = state.next().expect("first nonce");
        let second = state.next().expect("second nonce");

        assert_eq!(nonce_counter(&first), 0);
        assert_eq!(nonce_counter(&second), 1);
        assert_ne!(first, second);
    }

    #[test]
    fn send_nonce_counter_refuses_overflow() {
        let mut state = SendNonceState::with_salt([1; 8], [2; 8]);
        state.counter = u64::MAX;

        let error = state.next().expect_err("counter exhausted");

        assert!(error.to_string().contains("send nonce counter exhausted"));
    }

    #[test]
    fn replay_window_rejects_duplicate_nonce() {
        let mut window = ReplayWindow::default();
        let mut state = SendNonceState::with_salt([1; 8], [2; 8]);
        let nonce = state.next().expect("nonce");

        window.accept(&nonce).expect("first accept");
        let error = window.accept(&nonce).expect_err("replay");

        assert!(error.to_string().contains("replayed frame nonce"));
    }

    #[test]
    fn replay_window_rejects_stale_nonce() {
        let mut window = ReplayWindow::default();
        let mut state = SendNonceState::with_salt([1; 8], [2; 8]);
        let stale = state.next().expect("stale nonce");
        for _ in 0..REPLAY_WINDOW_BITS {
            let nonce = state.next().expect("newer nonce");
            window.accept(&nonce).expect("accept newer nonce");
        }

        let error = window.accept(&stale).expect_err("stale");

        assert!(error.to_string().contains("stale frame nonce"));
    }
}
