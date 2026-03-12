use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::serde_json;
use sha2::Digest;
use templar_common::oracle::redstone::FeedId;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    net::UnixListener,
    select,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

fn generate_socket_path() -> PathBuf {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or_else(
            |_| Path::new("/tmp/templar_redstone_bridge.sock").to_owned(),
            |t| {
                let d = hex::encode(&sha2::Sha256::digest(t.as_micros().to_le_bytes())[0..4]);
                Path::new(&format!("/tmp/templar_redstone_bridge_{d}.sock")).to_owned()
            },
        )
}

struct Request {
    send: oneshot::Sender<Result<String, String>>,
    method: IpcRequestMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct IpcRequest {
    id: u32,
    #[serde(flatten)]
    method: IpcRequestMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    crate = "near_sdk::serde",
    tag = "method",
    rename_all = "snake_case",
    content = "params"
)]
enum IpcRequestMethod {
    Fetch(Vec<FeedId>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct IpcResponse {
    id: u32,
    #[serde(flatten)]
    result: IpcResponseResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde", tag = "status", rename_all = "snake_case")]
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

struct BridgePaths<'a> {
    node: &'a Path,
    bridge: &'a Path,
    socket: PathBuf,
}

fn start_bridge(paths: BridgePaths<'_>, kill: watch::Sender<()>) -> JoinHandle<()> {
    use tokio::process::Command;

    let mut cmd = Command::new(paths.node);
    cmd.arg(paths.bridge);
    cmd.arg("--socket");
    cmd.arg(&paths.socket);
    cmd.arg("--data-service-id");
    cmd.arg("redstone-primary-prod");
    cmd.kill_on_drop(true);

    let mut on_kill = kill.subscribe();

    tokio::spawn(async move {
        let mut process = match cmd.spawn() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = ?e, "Failed to start RedStone bridge");
                let _ = kill.send(());
                return;
            }
        };

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

        let _ = std::fs::remove_file(&paths.socket);
    })
}

fn start_messenger(socket_path: PathBuf, kill: watch::Sender<()>) -> mpsc::Sender<Request> {
    let (send, mut recv) = mpsc::channel::<Request>(64);
    let mut on_kill = kill.subscribe();
    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!(error = ?e, "Failed to bind to socket");
            let _ = kill.send(());
            return send;
        }
    };

    tokio::spawn(async move {
        // Race acceptance with shutdown so we don't block forever on accept().
        let (socket, _address) = match select! {
            connection = listener.accept() => connection,
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification before accepting connection.");
                // Clean up socket file and exit early since we never accepted a connection.
                let _ = std::fs::remove_file(&socket_path);
                return;
            }
        } {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(error = ?e, "Failed to accept socket connection");
                let _ = kill.send(());
                let _ = std::fs::remove_file(&socket_path);
                return;
            }
        };

        let (read, mut write) = socket.into_split();
        let mut read = tokio::io::BufReader::new(read);
        let mut next_id = 0u32;
        let mut line = String::new();
        let mut pending = HashMap::<u32, oneshot::Sender<Result<String, String>>>::new();

        loop {
            select! {
                _ = on_kill.changed() => {
                    tracing::debug!("Received kill notification.");
                    break;
                },
                _ = read.read_line(&mut line) => {
                    tracing::debug!(line, "Received IPC message");
                    let received: IpcResponse = match serde_json::from_str(&line) {
                        Ok(r) => {r},
                        Err(e) => {
                            tracing::error!(line, error = ?e, "Failed deserializing response from bridge");
                            continue;
                        },
                    };
                    line.clear();

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

        let _ = std::fs::remove_file(&socket_path);
    });

    send
}

/// The bundled RedStone bridge JS source, embedded at compile time.
pub const BRIDGE_BUNDLE: &str = include_str!(concat!(env!("OUT_DIR"), "/bundle.js"));

/// Manages a Node.js child process and Unix socket IPC for communicating
/// with the RedStone bridge.
#[derive(Debug, Clone)]
pub struct Bridge {
    socket_path: PathBuf,
    /// Temp file holding the embedded JS bundle, cleaned up on drop.
    bundle_path: Arc<PathBuf>,
    #[allow(unused, reason = "Used for Drop implementation")]
    bridge_process: Arc<JoinHandle<()>>,
    bridge_send: mpsc::Sender<Request>,
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
        let bundle_path =
            std::env::temp_dir().join(format!("templar_redstone_bundle_{}.js", std::process::id()));
        std::fs::write(&bundle_path, BRIDGE_BUNDLE).map_err(|e| {
            BridgeError::Bundle(format!(
                "Failed to write bundle to {}: {e}",
                bundle_path.display()
            ))
        })?;

        let bundle_path = Arc::new(bundle_path);
        let socket_path = generate_socket_path();
        let bridge_send = start_messenger(socket_path.clone(), kill.clone());
        let bridge_process = Arc::new(start_bridge(
            BridgePaths {
                node: node_path,
                bridge: &bundle_path,
                socket: socket_path.clone(),
            },
            kill,
        ));

        Ok(Self {
            socket_path,
            bundle_path,
            bridge_process,
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
    pub async fn fetch(&self, feed_ids: Vec<FeedId>) -> Result<Vec<u8>, BridgeError> {
        let (send, recv) = oneshot::channel();
        self.bridge_send
            .send(Request {
                send,
                method: IpcRequestMethod::Fetch(feed_ids),
            })
            .await
            .map_err(|_| BridgeError::Send)?;
        let payload_hex = recv.await?.map_err(BridgeError::Bridge)?;

        Ok(hex::decode(&payload_hex)?)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(self.bundle_path.as_ref());
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
    #[error("Bundle error: {0}")]
    Bundle(String),
}
