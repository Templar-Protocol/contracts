use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use near_primitives::action::{Action, FunctionCallAction};
use near_sdk::{
    json_types::Base64VecU8,
    serde::{Deserialize, Serialize},
    serde_json::{self, json},
};
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

static SOCKET_PATH: &str = "/tmp/templar_redstone_bridge.sock";

#[derive(Debug, Clone)]
pub struct RedStoneSpec {
    config: args::RedStone,
    #[allow(unused, reason = "Used for Drop implementation")]
    bridge_process: Arc<JoinHandle<()>>,
    bridge_send: mpsc::Sender<Request>,
}

fn start_bridge(node_path: &Path, bridge_path: &Path, kill: watch::Sender<()>) -> JoinHandle<()> {
    use tokio::process::Command;

    let mut cmd = Command::new(node_path);
    cmd.arg(bridge_path);
    cmd.arg("--socket");
    cmd.arg(SOCKET_PATH);
    cmd.arg("--data-service-id");
    cmd.arg("redstone-primary-prod");
    cmd.kill_on_drop(true);

    let mut on_kill = kill.subscribe();

    tokio::spawn(async move {
        let mut process = cmd.spawn().unwrap();

        select! {
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification.");
                process.kill().await.unwrap();
            },
            status = process.wait() => {
                tracing::error!(?status, "RedStone bridge exited unexpectedly");
                kill.send(());
            }
        }

        let _ = std::fs::remove_file(SOCKET_PATH);
    })
}

pub struct Request {
    send: oneshot::Sender<Result<String, String>>,
    method: IpcRequestMethod,
}

impl Request {
    pub fn fetch(ids: Vec<String>) -> (Self, oneshot::Receiver<Result<String, String>>) {
        let (send, recv) = oneshot::channel();
        (
            Self {
                send,
                method: IpcRequestMethod::Fetch(ids),
            },
            recv,
        )
    }
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
    inner: IpcResponseInner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde", tag = "status", rename_all = "snake_case")]
enum IpcResponseInner {
    Success { data: String },
    Failure { message: String },
}

impl From<IpcResponseInner> for Result<String, String> {
    fn from(value: IpcResponseInner) -> Self {
        match value {
            IpcResponseInner::Success { data } => Ok(data),
            IpcResponseInner::Failure { message } => Err(message),
        }
    }
}

fn start_messenger(kill: watch::Sender<()>) -> mpsc::Sender<Request> {
    let (send, mut recv) = mpsc::channel::<Request>(64);
    let mut on_kill = kill.subscribe();
    let listener = UnixListener::bind(SOCKET_PATH).unwrap();

    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut read = tokio::io::BufReader::new(read);
        let mut next_id = 0u32;
        let mut buf = String::new();
        let mut pending = HashMap::<u32, oneshot::Sender<Result<String, String>>>::new();

        loop {
            select! {
                _ = on_kill.changed() => {
                    tracing::debug!("Received kill notification.");
                    break;
                },
                _ = read.read_line(&mut buf) => {
                    tracing::debug!(received = buf, "Received IPC message");
                    let received: IpcResponse = serde_json::from_str(&buf).unwrap();
                    buf.clear();
                    pending.remove(&received.id).unwrap().send(received.inner.into()).unwrap();
                },
                request = recv.recv() => {
                    let Some(request) = request else {
                        tracing::debug!("Sender dropped, exiting.");
                        break;
                    };

                    let id = next_id;
                    next_id += 1;
                    let ipc_request = IpcRequest { id, method: request.method };

                    pending.insert(id, request.send);

                    tracing::debug!(?ipc_request, "Sending IPC request");

                    write.write_all(&serde_json::to_vec(&ipc_request).unwrap()).await.unwrap();
                },
            }
        }

        let _ = std::fs::remove_file(SOCKET_PATH);
    });

    send
}

impl RedStoneSpec {
    pub fn new(config: args::RedStone, kill: watch::Sender<()>) -> Self {
        let bridge_send = start_messenger(kill.clone());
        let bridge_process = Arc::new(start_bridge(&config.nodejs_path, &config.bridge_path, kill));
        Self {
            config,
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
}

impl Drop for RedStoneSpec {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(SOCKET_PATH);
    }
}

impl Spec for RedStoneSpec {
    type FeedId = String;
    type Error = std::io::Error;

    fn name() -> &'static str {
        "redstone"
    }

    fn oracle_id(&self) -> &near_sdk::AccountIdRef {
        &self.config.oracle_id
    }

    fn refresh(&self) -> Duration {
        self.config.refresh
    }

    #[tracing::instrument(skip(self))]
    async fn update_actions(&self, feed_ids: &[Self::FeedId]) -> Result<Vec<Action>, Self::Error> {
        let (req, recv) = Request::fetch(feed_ids.to_vec());
        self.bridge_send.send(req).await.unwrap();
        let payload_string_hex = recv.await.unwrap().unwrap();
        let payload_vec = hex::decode(&payload_string_hex).unwrap();

        Ok(vec![FunctionCallAction {
            method_name: "write_prices".to_string(),
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
            update_deposit: NearToken::from_near(1).saturating_div(100),
            nodejs_path: "node".parse().unwrap(),
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
