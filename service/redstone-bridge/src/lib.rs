use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use templar_common::oracle::redstone::FeedId;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    net::UnixListener,
    process::Command,
    select,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

fn gen_temp_file_path(name: &str, extension: &str) -> TryDeleteOnDrop {
    let pid = std::process::id();
    #[allow(
        clippy::expect_used,
        reason = "system time before unix epoch is impossible"
    )]
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    TryDeleteOnDrop(std::env::temp_dir().join(format!("{name}_{pid}_{ts}.{extension}")))
}

#[derive(Debug, Clone)]
#[must_use]
struct TryDeleteOnDrop(PathBuf);

impl AsRef<Path> for TryDeleteOnDrop {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TryDeleteOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Debug)]
struct Request {
    send: oneshot::Sender<Result<String, String>>,
    method: IpcRequestMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IpcRequest {
    id: u32,
    #[serde(flatten)]
    method: IpcRequestMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", content = "params")]
enum IpcRequestMethod {
    Fetch(Vec<FeedId>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IpcResponse {
    id: u32,
    #[serde(flatten)]
    result: IpcResponseResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum IpcResponseResult {
    Success { data: String },
    Failure { message: String },
}

impl From<IpcResponseResult> for Result<String, String> {
    fn from(value: IpcResponseResult) -> Self {
        match value {
            IpcResponseResult::Success { data } => Ok(data),
            IpcResponseResult::Failure { message } => Err(message),
        }
    }
}

#[tracing::instrument(skip(kill))]
fn start_bridge(
    node: &Path,
    bridge: &Path,
    socket: &Path,
    kill: watch::Sender<()>,
) -> Result<JoinHandle<()>, std::io::Error> {
    let mut cmd = Command::new(node);
    cmd.arg(bridge);
    cmd.arg("--socket");
    cmd.arg(socket);
    cmd.arg("--data-service-id");
    cmd.arg("redstone-primary-prod");
    cmd.kill_on_drop(true);

    let mut process = cmd.spawn()?;

    let mut on_kill = kill.subscribe();

    Ok(tokio::spawn(async move {
        select! {
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification");
                if let Err(e) = process.kill().await {
                    tracing::error!(error = ?e, "Failed to kill RedStone bridge process");
                }
            },
            status = process.wait() => {
                tracing::error!(?status, "RedStone bridge exited unexpectedly");
                let _ = kill.send(());
            }
        }
    }))
}

#[tracing::instrument(skip(kill), name = "messenger")]
fn start_messenger(
    socket_path: &Path,
    kill: watch::Sender<()>,
) -> Result<mpsc::Sender<Request>, std::io::Error> {
    let listener = UnixListener::bind(socket_path)?;
    let (send, mut recv) = mpsc::channel::<Request>(64);
    let mut on_kill = kill.subscribe();

    tokio::spawn(async move {
        // Race acceptance with shutdown so we don't block forever on accept().
        let (socket, _address) = match select! {
            connection = listener.accept() => connection,
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification before accepting connection.");
                return;
            }
        } {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(error = ?e, "Failed to accept socket connection");
                let _ = kill.send(());
                return;
            }
        };

        let (read, mut write) = socket.into_split();
        let mut read = tokio::io::BufReader::new(read).lines();
        let mut next_id = 0u32;
        let mut pending = HashMap::<u32, oneshot::Sender<Result<String, String>>>::new();

        loop {
            select! {
                _ = on_kill.changed() => {
                    tracing::debug!("Received kill notification.");
                    break;
                },
                line = read.next_line() => {
                    let line = match line {
                        Ok(Some(line)) => line,
                        Ok(None) => {
                            tracing::error!("Unexpected EOF from socket");
                            let _ = kill.send(());
                            break;
                        },
                        Err(e) => {
                            tracing::error!(error = ?e, "Failed reading line from bridge socket");
                            continue;
                        },
                    };
                    tracing::debug!(line, "Received IPC message");
                    let received: IpcResponse = match serde_json::from_str(&line) {
                        Ok(r) => {r},
                        Err(e) => {
                            tracing::error!(line, error = ?e, "Failed deserializing response from bridge");
                            continue;
                        },
                    };

                    if let Some(sender) = pending.remove(&received.id) {
                        if let Err(result) = sender.send(received.result.into()) {
                            tracing::warn!(?result, "Bridge message receiver dropped");
                        }
                    } else {
                        tracing::error!(id = received.id, ?received, "Response from bridge has unknown ID");
                    }
                },
                request = recv.recv() => {
                    let Some(request) = request else {
                        tracing::debug!("Sender dropped, exiting");
                        let _ = kill.send(());
                        break;
                    };

                    let id = next_id;
                    next_id += 1;
                    let ipc_request = IpcRequest { id, method: request.method };

                    pending.insert(id, request.send);

                    tracing::debug!(?ipc_request, "Sending IPC request");

                    let serialized = match serde_json::to_vec(&ipc_request) {
                        Ok(mut s) => {
                            // Newline delimiter
                            s.push(b'\n');
                            s
                        },
                        Err(e) => {
                            tracing::error!(error = ?e, "IPC request serialization");
                            let _ = pending.remove(&id);
                            continue;
                        }
                    };

                    match write.write_all(&serialized).await {
                        Ok(()) => {},
                        Err(e) => {
                            tracing::error!(error = ?e, "Error writing to socket");
                            let _ = pending.remove(&id);
                        }
                    }
                },
            }
        }
    });

    Ok(send)
}

/// The bundled RedStone bridge JS source, embedded at compile time.
pub const BRIDGE_BUNDLE: &str = include_str!(concat!(env!("OUT_DIR"), "/bundle.js"));

/// Manages a Node.js child process and Unix socket IPC for communicating
/// with the RedStone bridge.
#[derive(Debug, Clone)]
pub struct Bridge {
    _cleanup: Arc<Cleanup>,
    bridge_send: mpsc::Sender<Request>,
}

#[derive(Debug)]
struct Cleanup {
    _bridge_process: JoinHandle<()>,
    _socket_path: TryDeleteOnDrop,
    _bundle_path: TryDeleteOnDrop,
}

impl Bridge {
    /// Create a new bridge instance using the embedded JS bundle.
    ///
    /// Writes the compiled bundle to a temporary file and spawns a Node.js
    /// process. The temp file is cleaned up when the `Bridge` is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if the temp file cannot be written.
    pub fn new(node_path: &Path, kill: watch::Sender<()>) -> Result<Self, BridgeError> {
        let bundle_path = gen_temp_file_path("templar_redstone_bundle", "js");
        std::fs::write(&bundle_path, BRIDGE_BUNDLE).map_err(BridgeError::WriteBundle)?;

        let socket_path = gen_temp_file_path("templar_redstone_bridge", "sock");
        let bridge_send = start_messenger(socket_path.as_ref(), kill.clone())
            .map_err(BridgeError::StartMessenger)?;
        let bridge_process =
            start_bridge(node_path, bundle_path.as_ref(), socket_path.as_ref(), kill)
                .map_err(BridgeError::StartBridge)?;

        Ok(Self {
            _cleanup: Arc::new(Cleanup {
                _socket_path: socket_path,
                _bundle_path: bundle_path,
                _bridge_process: bridge_process,
            }),
            bridge_send,
        })
    }

    /// Fetch update payloads for given feed IDs from the RedStone bridge.
    ///
    /// # Errors
    ///
    /// - Communication with the bridge.
    /// - Communication between the bridge and RedStone nodes.
    /// - Deserialization of response from the bridge.
    #[tracing::instrument(skip(self))]
    pub async fn fetch(&self, feed_ids: Vec<FeedId>) -> Result<Vec<u8>, BridgeError> {
        let (send, recv) = oneshot::channel();
        let request = Request {
            send,
            method: IpcRequestMethod::Fetch(feed_ids),
        };
        tracing::debug!(?request);
        self.bridge_send.send(request).await.map_err(|e| {
            tracing::warn!("Failed to send to bridge: {}", e);
            BridgeError::Send
        })?;
        let payload_hex = recv.await?.map_err(BridgeError::Bridge)?;

        Ok(hex::decode(&payload_hex)?)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum BridgeError {
    #[error("Failed to send to bridge")]
    Send,
    #[error("Failed to receive from bridge: {0}")]
    Recv(#[from] oneshot::error::RecvError),
    #[error("Bridge returned error: {0}")]
    Bridge(String),
    #[error("Data encoding error: {0}")]
    Data(#[from] hex::FromHexError),
    #[error("Failed to write bundle: {0}")]
    WriteBundle(#[source] std::io::Error),
    #[error("Failed to start messenger: {0}")]
    StartMessenger(#[source] std::io::Error),
    #[error("Failed to start bridge: {0}")]
    StartBridge(#[source] std::io::Error),
}
