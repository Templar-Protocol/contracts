use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use near_primitives::action::{Action, FunctionCallAction};
use near_sdk::{
    json_types::Base64VecU8,
    serde::{Deserialize, Serialize},
    serde_json::{self, json},
};
use sha2::Digest;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    net::UnixListener,
    select,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use crate::{
    app::args,
    cache::Cache,
    client::{near::Near, oracle::Handle},
};

use super::Spec;

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

#[derive(Debug, Clone)]
pub struct RedStoneSpec {
    config: args::RedStone,
    socket_path: PathBuf,
    #[allow(unused, reason = "Used for Drop implementation")]
    bridge_process: Arc<JoinHandle<()>>,
    bridge_send: mpsc::Sender<Request>,
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

pub struct Request {
    send: oneshot::Sender<Result<String, String>>,
    method: IpcRequestMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct IpcRequest {
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
    Fetch(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct IpcResponse {
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
        let (socket, _address) = match listener.accept().await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(error = ?e, "Failed to accept socket connection");
                let _ = kill.send(());
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

impl RedStoneSpec {
    pub fn new(config: args::RedStone, kill: watch::Sender<()>) -> Self {
        let socket_path = generate_socket_path();
        let bridge_send = start_messenger(socket_path.clone(), kill.clone());
        let bridge_process = Arc::new(start_bridge(
            BridgePaths {
                node: &config.node_path,
                bridge: &config.bridge_path,
                socket: socket_path.clone(),
            },
            kill,
        ));
        Self {
            config,
            socket_path,
            bridge_process,
            bridge_send,
        }
    }

    pub fn handle(
        config: args::RedStone,
        near: Near,
        cache: Cache,
        kill: watch::Sender<()>,
    ) -> Handle<Self> {
        Handle::new(Arc::new(Self::new(config, kill.clone())), near, cache, kill)
    }

    /// Fetch update payloads for given feed IDs from the RedStone bridge.
    ///
    /// # Errors
    ///
    /// - Communication with the bridge.
    /// - Communication between the bridge and RedStone nodes.
    /// - Deserialization of response from the bridge.
    pub async fn fetch(&self, feed_ids: Vec<String>) -> Result<Vec<u8>, RequestError> {
        let (send, recv) = oneshot::channel();
        self.bridge_send
            .send(Request {
                send,
                method: IpcRequestMethod::Fetch(feed_ids),
            })
            .await?;
        let payload_hex = recv.await?.map_err(RequestError::Bridge)?;

        Ok(hex::decode(&payload_hex)?)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum RequestError {
    #[error("Failed to send to bridge: {0}")]
    Send(#[from] mpsc::error::SendError<Request>),
    #[error("Failed to receive from bridge: {0}")]
    Recv(#[from] oneshot::error::RecvError),
    #[error("Bridge returned error: {0}")]
    Bridge(String),
    #[error("Data encoding error: {0}")]
    Data(#[from] hex::FromHexError),
}

impl Drop for RedStoneSpec {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Spec for RedStoneSpec {
    type FeedId = String;
    type Error = RequestError;

    fn name() -> &'static str {
        "RedStone"
    }

    fn oracle_id(&self) -> &near_sdk::AccountIdRef {
        &self.config.oracle_id
    }

    fn refresh(&self) -> Duration {
        self.config.refresh
    }

    #[tracing::instrument(skip(self))]
    async fn update_actions(&self, feed_ids: &[Self::FeedId]) -> Result<Vec<Action>, Self::Error> {
        let payload_vec = self.fetch(feed_ids.to_vec()).await?;

        Ok(vec![FunctionCallAction {
            method_name: "write_prices".to_string(),
            #[allow(clippy::unwrap_used, reason = "This serialization is infallible")]
            args: serde_json::to_vec(&json!({
                "feed_ids": feed_ids,
                "payload": Base64VecU8(payload_vec),
            }))
            .unwrap(),
            gas: self.config.update_gas.as_gas(),
            deposit: self.config.update_deposit.as_yoctonear(),
        }
        .into()])
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::NearToken;

    use crate::app::args;

    use super::*;

    #[tokio::test]
    async fn update_actions() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .init();

        let redstone_args = args::RedStone {
            refresh: Duration::from_secs(25),
            oracle_id: "does_not_exist.near".parse().unwrap(),
            update_gas: near_sdk::Gas::from_tgas(300),
            update_deposit: NearToken::from_near(0),
            node_path: Path::new("node").to_owned(),
            bridge_path: "./redstone-bridge/dist/index.js".parse().unwrap(),
        };

        let kill = watch::Sender::default();

        let spec = RedStoneSpec::new(redstone_args, kill.clone());

        let t = spec
            .update_actions(&["ETH".to_string(), "BTC".to_string()])
            .await
            .unwrap();

        eprintln!("{t:?}");

        kill.send(()).unwrap();
    }
}
