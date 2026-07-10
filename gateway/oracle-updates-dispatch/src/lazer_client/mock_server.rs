//! An in-process stand-in for the Pyth Lazer stream, so the source's connection
//! lifecycle is testable without the live endpoint. Tests drive the accepted
//! [`ServerConn`] directly: read frames, answer with protocol responses, or drop
//! it to simulate a server-side close.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pyth_lazer_protocol::api::{WsRequest, WsResponse};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};
use url::Url;

/// One accepted client connection, driven directly by the test.
pub(crate) type ServerConn = WebSocketStream<TcpStream>;

pub(crate) struct MockLazer {
    url: Url,
    connections: mpsc::UnboundedReceiver<ServerConn>,
    accept_task: JoinHandle<()>,
}

impl Drop for MockLazer {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

impl MockLazer {
    pub(crate) async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock server should bind a loopback port");
        let addr = listener
            .local_addr()
            .expect("bound listener should have an address");
        let (connection_tx, connections) = mpsc::unbounded_channel();

        let accept_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let Ok(websocket) = accept_async(stream).await else {
                    continue;
                };
                if connection_tx.send(websocket).is_err() {
                    break;
                }
            }
        });

        Self {
            url: format!("ws://{addr}/v1/stream")
                .parse()
                .expect("loopback address should form a valid ws:// url"),
            connections,
            accept_task,
        }
    }

    pub(crate) fn url(&self) -> Url {
        self.url.clone()
    }

    /// Wait for the source to connect, failing the test if it does not.
    pub(crate) async fn accept(&mut self) -> ServerConn {
        self.accept_within(Duration::from_secs(5))
            .await
            .expect("source should have connected")
    }

    /// `None` means the source did not connect — the assertion for the idle case.
    pub(crate) async fn accept_within(&mut self, within: Duration) -> Option<ServerConn> {
        timeout(within, self.connections.recv())
            .await
            .ok()
            .flatten()
    }
}

/// Read the next client frame, decoded as a protocol request.
pub(crate) async fn next_request(connection: &mut ServerConn) -> WsRequest {
    let message = timeout(Duration::from_secs(5), connection.next())
        .await
        .expect("client should send a frame")
        .expect("stream should not end")
        .expect("frame should not error");
    let Message::Text(text) = message else {
        panic!("expected a text frame, got {message:?}");
    };
    serde_json::from_str(text.as_ref()).expect("client frame should be a protocol request")
}

/// The feed ids of the next `subscribe` frame, failing on any other request.
pub(crate) async fn next_subscribed_feed_ids(connection: &mut ServerConn) -> Vec<u32> {
    match next_request(connection).await {
        WsRequest::Subscribe(request) => request
            .params
            .price_feed_ids
            .clone()
            .expect("subscribe frame should carry price feed ids")
            .into_iter()
            .map(|feed| feed.0)
            .collect(),
        other @ WsRequest::Unsubscribe(_) => panic!("expected a subscribe request, got {other:?}"),
    }
}

pub(crate) async fn send(connection: &mut ServerConn, response: &WsResponse) {
    let text = serde_json::to_string(response).expect("response should serialize");
    connection
        .send(Message::Text(text.into()))
        .await
        .expect("mock server should send a frame");
}
