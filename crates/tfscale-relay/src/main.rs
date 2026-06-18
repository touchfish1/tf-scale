use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tfscale_core::protocol::RelayMessage;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc},
};
use tracing::{info, warn};

#[derive(Debug, Parser)]
#[command(name = "tfscale-relay", version, about = "tf-scale relay service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value = "0.0.0.0:9443")]
        listen: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { listen } => serve(listen).await?,
    }

    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

async fn serve(listen: String) -> Result<()> {
    let addr: SocketAddr = listen.parse()?;
    let listener = TcpListener::bind(addr).await?;
    let state = RelayState::default();
    info!(%addr, "relay listening");

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_session(state, stream).await {
                warn!(%error, %peer_addr, "relay session closed");
            }
        });
    }
}

#[derive(Clone, Default)]
struct RelayState {
    sessions: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<RelayMessage>>>>,
    routed_frames: Arc<AtomicU64>,
    dropped_frames: Arc<AtomicU64>,
}

impl RelayState {
    async fn register(
        &self,
        device_id: String,
        sender: mpsc::UnboundedSender<RelayMessage>,
    ) -> Option<mpsc::UnboundedSender<RelayMessage>> {
        self.sessions.lock().await.insert(device_id, sender)
    }

    async fn unregister(&self, device_id: &str) {
        self.sessions.lock().await.remove(device_id);
    }

    async fn route_frame(&self, frame: RelayMessage) -> bool {
        let RelayMessage::Frame {
            destination_device_id,
            ..
        } = &frame
        else {
            return false;
        };
        let sender = self
            .sessions
            .lock()
            .await
            .get(destination_device_id)
            .cloned();
        if let Some(sender) = sender {
            if sender.send(frame).is_ok() {
                self.routed_frames.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }

        self.dropped_frames.fetch_add(1, Ordering::Relaxed);
        false
    }

    #[cfg(test)]
    fn counters(&self) -> RelayCounters {
        RelayCounters {
            routed_frames: self.routed_frames.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RelayCounters {
    routed_frames: u64,
    dropped_frames: u64,
}

async fn handle_session(state: RelayState, stream: TcpStream) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let Some(line) = lines.next_line().await? else {
        return Ok(());
    };
    let RelayMessage::Register { device_id, .. } = serde_json::from_str(&line)? else {
        write_message(
            &mut writer,
            &RelayMessage::Error {
                message: "first relay message must be register".to_string(),
            },
        )
        .await?;
        return Ok(());
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    state.register(device_id.clone(), tx).await;
    loop {
        tokio::select! {
            inbound = lines.next_line() => {
                let Some(line) = inbound? else {
                    break;
                };
                let message: RelayMessage = serde_json::from_str(&line)?;
                match message {
                    RelayMessage::Frame { source_device_id, destination_device_id, payload } => {
                        let delivered = state.route_frame(RelayMessage::Frame {
                            source_device_id: source_device_id.clone(),
                            destination_device_id: destination_device_id.clone(),
                            payload,
                        }).await;
                        if !delivered {
                            write_message(
                                &mut writer,
                                &RelayMessage::Error {
                                    message: format!("unknown destination device: {destination_device_id}"),
                                },
                            ).await?;
                        }
                    }
                    RelayMessage::Register { .. } => {
                        write_message(
                            &mut writer,
                            &RelayMessage::Error {
                                message: "session is already registered".to_string(),
                            },
                        ).await?;
                    }
                    RelayMessage::Error { .. } => {}
                }
            }
            outbound = rx.recv() => {
                let Some(message) = outbound else {
                    break;
                };
                write_message(&mut writer, &message).await?;
            }
        }
    }

    state.unregister(&device_id).await;
    Ok(())
}

async fn write_message(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    message: &RelayMessage,
) -> Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routes_frame_to_registered_destination() {
        let state = RelayState::default();
        let (source_tx, _source_rx) = mpsc::unbounded_channel();
        let (destination_tx, mut destination_rx) = mpsc::unbounded_channel();
        state
            .register("dev_a".to_string(), source_tx)
            .await
            .expect_none("first source registration");
        state
            .register("dev_b".to_string(), destination_tx)
            .await
            .expect_none("first destination registration");

        let delivered = state
            .route_frame(RelayMessage::Frame {
                source_device_id: "dev_a".to_string(),
                destination_device_id: "dev_b".to_string(),
                payload: "encrypted-frame".to_string(),
            })
            .await;

        assert!(delivered);
        let RelayMessage::Frame {
            source_device_id,
            destination_device_id,
            payload,
        } = destination_rx.recv().await.expect("routed frame")
        else {
            panic!("expected frame");
        };
        assert_eq!(source_device_id, "dev_a");
        assert_eq!(destination_device_id, "dev_b");
        assert_eq!(payload, "encrypted-frame");
        assert_eq!(state.counters().routed_frames, 1);
        assert_eq!(state.counters().dropped_frames, 0);
    }

    #[tokio::test]
    async fn drops_frame_for_unknown_destination() {
        let state = RelayState::default();

        let delivered = state
            .route_frame(RelayMessage::Frame {
                source_device_id: "dev_a".to_string(),
                destination_device_id: "dev_missing".to_string(),
                payload: "encrypted-frame".to_string(),
            })
            .await;

        assert!(!delivered);
        assert_eq!(state.counters().routed_frames, 0);
        assert_eq!(state.counters().dropped_frames, 1);
    }

    trait OptionExt<T> {
        fn expect_none(self, message: &str);
    }

    impl<T> OptionExt<T> for Option<T> {
        fn expect_none(self, message: &str) {
            assert!(self.is_none(), "{message}");
        }
    }
}
