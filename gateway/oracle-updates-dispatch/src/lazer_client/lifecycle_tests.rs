//! Connection-lifecycle tests for [`LazerPayloadSource`], driven against the in-process
//! [`mock_server`]. The live endpoint closes a subscription-less connection after 60s, so
//! when the source connects is a correctness question, not a detail.

use std::time::Duration;

use futures::StreamExt as _;
use pyth_lazer_protocol::api::{SubscriptionErrorResponse, SubscriptionId, WsResponse};
use templar_gateway_core::OraclePayloadSource;
use tokio::{
    task::JoinHandle,
    time::{timeout, Instant},
};

use super::*;
use crate::lazer_client::{
    fixtures::{fixture_bytes, fixture_response},
    mock_server::{next_subscribed_feed_ids, send, MockLazer, ServerConn},
};

/// A feed carried by the captured fixture payload.
const FIXTURE_FEED: u32 = 7;

/// Generous enough that a failure means "never happened", not "was slow".
const PATIENCE: Duration = Duration::from_secs(5);

fn source_for(server: &MockLazer, max_payload_age: Duration) -> LazerPayloadSource {
    LazerPayloadSource::spawn(LazerSourceConfig::for_mock_server(
        server.url(),
        max_payload_age,
    ))
}

fn source_with_idle_timeout(
    server: &MockLazer,
    max_payload_age: Duration,
    idle_timeout: Duration,
) -> LazerPayloadSource {
    LazerPayloadSource::spawn_with_idle_timeout(
        LazerSourceConfig::for_mock_server(server.url(), max_payload_age),
        idle_timeout,
    )
}

/// Pointed at a port nothing listens on: for tests that drive the task directly.
fn offline_task() -> StreamTask {
    StreamTask::new(
        LazerSourceConfig::for_mock_server(
            "ws://127.0.0.1:1/v1/stream".parse().expect("valid url"),
            Duration::from_secs(5),
        ),
        STREAM_IDLE_TIMEOUT,
    )
}

/// Off the current task, so the test can drive the server while the fetch is in flight.
fn spawn_fetch(source: &LazerPayloadSource) -> JoinHandle<LazerResult<Vec<u8>>> {
    let probe = source.clone();
    tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await })
}

/// Unwrap a spawned fetch through all three of its failure modes.
async fn payload_of(fetch: JoinHandle<LazerResult<Vec<u8>>>) -> Vec<u8> {
    timeout(PATIENCE, fetch)
        .await
        .expect("fetch should not time out")
        .expect("fetch task should not panic")
        .expect("fetch should return a payload")
}

/// Accept a connection, expect a subscribe frame for `FIXTURE_FEED`, and answer
/// with the fixture payload.
async fn accept_and_serve(server: &mut MockLazer) -> ServerConn {
    let mut connection = server.accept().await;
    let feed_ids = next_subscribed_feed_ids(&mut connection).await;
    assert_eq!(feed_ids, vec![FIXTURE_FEED]);
    send(&mut connection, &fixture_response(SubscriptionId(1))).await;
    connection
}

/// Drive the source to one live subscription with a payload cached, returning the
/// connection serving it.
async fn warm_up(source: &LazerPayloadSource, server: &mut MockLazer) -> ServerConn {
    let fetch = spawn_fetch(source);
    let connection = accept_and_serve(server).await;
    payload_of(fetch).await;
    connection
}

/// An eager connect becomes a permanent reconnect loop against the live endpoint's
/// 60s reap of subscription-less connections.
#[tokio::test]
async fn does_not_connect_until_a_payload_is_requested() {
    let mut server = MockLazer::start().await;
    let _source = source_for(&server, Duration::from_secs(5));

    let connection = server.accept_within(Duration::from_millis(500)).await;

    assert!(
        connection.is_none(),
        "source must not connect until a payload is requested"
    );
}

#[tokio::test]
async fn first_fetch_connects_subscribes_and_returns_the_payload() {
    let mut server = MockLazer::start().await;
    let source = source_for(&server, Duration::from_secs(5));

    let fetch = spawn_fetch(&source);
    let _connection = accept_and_serve(&mut server).await;

    assert_eq!(payload_of(fetch).await, fixture_bytes());
}

/// A fetch arriving during reconnect backoff must cancel the sleep. Two disconnects push
/// the backoff to ~2s; the reconnect must beat that by a wide margin.
#[tokio::test]
async fn fetch_during_reconnect_backoff_connects_immediately() {
    let mut server = MockLazer::start().await;
    // A short freshness bound so the cached payload is stale by the time the
    // second fetch runs, forcing it to wait on the stream rather than the cache.
    let source = source_for(&server, Duration::from_millis(200));
    let connection = warm_up(&source, &mut server).await;

    // Server-side close, twice: backoff grows to ~2s.
    drop(connection);
    let connection = server.accept().await;
    drop(connection);

    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = Instant::now();
    let fetch = spawn_fetch(&source);
    let _connection = accept_and_serve(&mut server).await;
    let reconnect_delay = started.elapsed();
    payload_of(fetch).await;

    assert!(
        reconnect_delay < Duration::from_millis(700),
        "a fetch during backoff must cancel the sleep and connect at once, took {reconnect_delay:?}"
    );
}

/// A server-side close must resubscribe every live subscription, unprompted.
#[tokio::test]
async fn reconnect_replays_active_subscriptions() {
    let mut server = MockLazer::start().await;
    let source = source_for(&server, Duration::from_secs(5));
    let connection = warm_up(&source, &mut server).await;

    drop(connection);

    let mut reconnected = server.accept().await;
    let feed_ids = next_subscribed_feed_ids(&mut reconnected).await;

    assert_eq!(
        feed_ids,
        vec![FIXTURE_FEED],
        "reconnect must replay the live subscription"
    );
}

/// A rejection must surface as an error, not a wait that expires on the fetch timeout.
#[tokio::test]
async fn subscription_error_fails_the_fetch() {
    let mut server = MockLazer::start().await;
    let source = source_for(&server, Duration::from_secs(5));

    let fetch = spawn_fetch(&source);
    let mut connection = server.accept().await;
    let _ = next_subscribed_feed_ids(&mut connection).await;
    send(
        &mut connection,
        &WsResponse::SubscriptionError(SubscriptionErrorResponse {
            subscription_id: SubscriptionId(1),
            error: "unknown feed id".to_owned(),
        }),
    )
    .await;

    let error = timeout(Duration::from_secs(2), fetch)
        .await
        .expect("a rejected subscription must fail fast, not wait for the fetch timeout")
        .expect("fetch task should not panic")
        .expect_err("a rejected subscription must fail the fetch");

    assert!(
        matches!(error, LazerClientError::SubscriptionFailed(ref message) if message.contains("unknown feed id")),
        "expected the server's rejection detail, got {error:?}"
    );

    // No subscription left, so the connection must be dropped. `source` stays alive, so
    // the close cannot come from the task guard aborting a dropped source.
    let closed = timeout(Duration::from_secs(2), connection.next())
        .await
        .expect("source should drop a connection with no subscriptions");
    assert!(
        matches!(closed, None | Some(Err(_))),
        "expected the connection to close, got {closed:?}"
    );

    drop(source);
}

/// A payload older than `max_payload_age` must neither be returned nor fail the fetch:
/// the next one is a channel interval away, so the fetch waits.
#[tokio::test]
async fn stale_cached_payload_waits_for_a_fresh_one() {
    let mut server = MockLazer::start().await;
    // Production idle timeout: the socket stays up, so the fetch waits on the stream.
    let source = source_for(&server, Duration::from_millis(200));
    let mut connection = warm_up(&source, &mut server).await;

    tokio::time::sleep(Duration::from_millis(400)).await;

    let mut fetch = spawn_fetch(&source);
    assert!(
        timeout(Duration::from_millis(100), &mut fetch)
            .await
            .is_err(),
        "a stale cached payload must not be returned, and must not fail the fetch"
    );

    send(&mut connection, &fixture_response(SubscriptionId(1))).await;

    assert_eq!(payload_of(fetch).await, fixture_bytes());
}

/// A silent websocket must trip the idle timeout even under fetch traffic: every fetch
/// messages the task, so a timer rebuilt per `select!` iteration would reset forever.
#[tokio::test]
async fn silent_stream_reconnects_despite_fetch_traffic() {
    let mut server = MockLazer::start().await;
    let source = source_with_idle_timeout(
        &server,
        Duration::from_millis(100),
        Duration::from_millis(500),
    );
    // Held open, then silent forever: the server never sends another frame.
    let _silent = warm_up(&source, &mut server).await;

    let poller = tokio::spawn(async move {
        for _ in 0..40_u32 {
            spawn_fetch(&source);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    let reconnected = server.accept_within(Duration::from_secs(3)).await;
    poller.abort();

    assert!(
        reconnected.is_some(),
        "a silent stream must trip the idle timeout and reconnect, even under fetch traffic"
    );
}

/// A socket that goes silent without closing must be detected, reconnected, and replayed
/// inside one fetch's deadline, or the fetch fails spuriously.
#[tokio::test]
async fn a_fetch_survives_a_silent_socket() {
    let mut server = MockLazer::start().await;
    let source = source_with_idle_timeout(
        &server,
        Duration::from_millis(100),
        Duration::from_millis(300),
    );
    let _silent = warm_up(&source, &mut server).await;

    // Let the cached payload go stale while the socket stays open and silent.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let fetch = spawn_fetch(&source);
    // The idle timer fires, the task reconnects and replays; serve the fetch there.
    let _reconnected = accept_and_serve(&mut server).await;

    assert_eq!(payload_of(fetch).await, fixture_bytes());
}

/// An abandoned request registers nothing, so the task must keep waiting rather than open
/// a subscription-less websocket — and stall a real fetch behind that connect.
#[tokio::test]
async fn an_abandoned_first_request_does_not_connect() {
    let mut server = MockLazer::start().await;
    let (requests_tx, requests_rx) = mpsc::channel(4);
    let task = tokio::spawn(
        StreamTask::new(
            LazerSourceConfig::for_mock_server(server.url(), Duration::from_secs(5)),
            STREAM_IDLE_TIMEOUT,
        )
        .run(requests_rx),
    );

    let (slot_tx, slot_rx) = oneshot::channel();
    drop(slot_rx);
    requests_tx
        .send(SubscribeRequest {
            feed_ids: [FIXTURE_FEED].into_iter().collect(),
            slot_tx,
        })
        .await
        .expect("stream task should accept the request");

    let connection = server.accept_within(Duration::from_millis(500)).await;
    task.abort();

    assert!(
        connection.is_none(),
        "an abandoned request must not open a subscription-less websocket"
    );
}

/// A requester that timed out has dropped its receiver. Registering it anyway would strand
/// a subscription nothing waits on, and at capacity evict a live one to make room.
#[test]
fn an_abandoned_request_registers_nothing() {
    let mut task = offline_task();
    let (slot_tx, slot_rx) = oneshot::channel();
    drop(slot_rx);

    let registration = task.register(SubscribeRequest {
        feed_ids: [FIXTURE_FEED].into_iter().collect(),
        slot_tx,
    });

    assert!(registration.new_subscription.is_none());
    assert!(registration.evicted.is_empty());
    assert!(
        task.subscriptions.is_empty(),
        "an abandoned request must not leave a subscription behind"
    );
}

/// `FETCH_TIMEOUT` is summed assuming the first reconnect after a payload sleeps only
/// `INITIAL_RECONNECT_BACKOFF`. That assumption is this reset.
#[test]
fn a_payload_resets_the_reconnect_backoff() {
    let mut task = offline_task();
    let (slot_tx, _slot_rx) = oneshot::channel();
    let subscription_id = task
        .register(SubscribeRequest {
            feed_ids: [FIXTURE_FEED].into_iter().collect(),
            slot_tx,
        })
        .new_subscription
        .expect("a live requester registers a subscription");

    task.backoff = MAX_RECONNECT_BACKOFF;
    task.cache_payload(DecodedLazerPayload {
        subscription_id,
        bytes: fixture_bytes(),
        feed_ids: [FIXTURE_FEED].into_iter().collect(),
    });

    assert_eq!(
        task.backoff, INITIAL_RECONNECT_BACKOFF,
        "a payload must reset the backoff, or FETCH_TIMEOUT's budget is wrong"
    );
}
