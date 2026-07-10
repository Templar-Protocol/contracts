//! Connection-lifecycle tests for [`LazerPayloadSource`], driven against the
//! in-process [`mock_server`].
//!
//! The production Lazer endpoint closes any connection carrying no subscription
//! after exactly 60s. A source that connects before anything subscribes therefore
//! reconnects forever, and a fetch arriving while it sleeps in reconnect backoff
//! is starved. Both properties are asserted here.

use std::time::Duration;

use futures::StreamExt as _;
use pyth_lazer_protocol::api::{SubscriptionErrorResponse, SubscriptionId, WsResponse};
use templar_gateway_core::OraclePayloadSource;
use tokio::time::{timeout, Instant};

use super::*;
use crate::lazer_client::{
    fixtures::{fixture_bytes, fixture_response},
    mock_server::{next_subscribed_feed_ids, send, MockLazer, ServerConn},
};

/// A feed carried by the captured fixture payload.
const FIXTURE_FEED: u32 = 7;

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
    LazerPayloadSource::spawn(
        LazerSourceConfig::for_mock_server(server.url(), max_payload_age)
            .with_idle_timeout(idle_timeout),
    )
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

/// The source must not hold a websocket open before anything subscribes: the
/// live endpoint reaps a subscription-less connection after 60s, so an eager
/// connect becomes a permanent reconnect loop.
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

    let fetch = tokio::spawn(async move { source.fetch_payload(&[FIXTURE_FEED]).await });
    let _connection = accept_and_serve(&mut server).await;

    let payload = timeout(Duration::from_secs(5), fetch)
        .await
        .expect("fetch should not time out")
        .expect("fetch task should not panic")
        .expect("fetch should return the payload");

    assert_eq!(payload, fixture_bytes());
}

/// A fetch arriving while the source sleeps in reconnect backoff must connect
/// immediately rather than waiting out the sleep. Two disconnects push the
/// backoff to ~2s; the reconnect must beat that by a wide margin.
#[tokio::test]
async fn fetch_during_reconnect_backoff_connects_immediately() {
    let mut server = MockLazer::start().await;
    // A short freshness bound so the cached payload is stale by the time the
    // second fetch runs, forcing it to wait on the stream rather than the cache.
    let source = source_for(&server, Duration::from_millis(200));

    let probe = source.clone();
    let fetch = tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await });
    let connection = accept_and_serve(&mut server).await;
    timeout(Duration::from_secs(5), fetch)
        .await
        .expect("first fetch should not time out")
        .expect("fetch task should not panic")
        .expect("first fetch should return the payload");

    // Server-side close, twice: backoff grows to ~2s.
    drop(connection);
    let connection = server.accept().await;
    drop(connection);

    tokio::time::sleep(Duration::from_millis(300)).await;

    let probe = source.clone();
    let started = Instant::now();
    let fetch = tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await });
    let _connection = accept_and_serve(&mut server).await;
    let reconnect_delay = started.elapsed();

    timeout(Duration::from_secs(5), fetch)
        .await
        .expect("fetch should not time out")
        .expect("fetch task should not panic")
        .expect("fetch should return the payload");

    assert!(
        reconnect_delay < Duration::from_millis(700),
        "a fetch during backoff must cancel the sleep and connect at once, took {reconnect_delay:?}"
    );
}

/// After a server-side close the source must resubscribe every live
/// subscription, without any new fetch prompting it.
#[tokio::test]
async fn reconnect_replays_active_subscriptions() {
    let mut server = MockLazer::start().await;
    let source = source_for(&server, Duration::from_secs(5));

    let probe = source.clone();
    let fetch = tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await });
    let connection = accept_and_serve(&mut server).await;
    timeout(Duration::from_secs(5), fetch)
        .await
        .expect("fetch should not time out")
        .expect("fetch task should not panic")
        .expect("fetch should return the payload");

    drop(connection);

    let mut reconnected = server.accept().await;
    let feed_ids = next_subscribed_feed_ids(&mut reconnected).await;

    assert_eq!(
        feed_ids,
        vec![FIXTURE_FEED],
        "reconnect must replay the live subscription"
    );
}

/// A server-side subscription rejection must surface as an error, not as a
/// silent wait that expires on the fetch timeout.
#[tokio::test]
async fn subscription_error_fails_the_fetch() {
    let mut server = MockLazer::start().await;
    let source = source_for(&server, Duration::from_secs(5));

    let probe = source.clone();
    let fetch = tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await });
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

    // The rejection left no subscription, so the connection has no purpose: the
    // source must drop it rather than idle until the server closes it. `source`
    // stays alive here, so the close can only come from that invariant and not
    // from the task guard aborting a dropped source.
    let closed = timeout(Duration::from_secs(2), connection.next())
        .await
        .expect("source should drop a connection with no subscriptions");
    assert!(
        matches!(closed, None | Some(Err(_))),
        "expected the connection to close, got {closed:?}"
    );

    drop(source);
}

/// A cached payload older than `max_payload_age` must not be returned, and must
/// not fail the fetch either: the next payload is ~200ms away on a live stream,
/// so the fetch waits for it.
#[tokio::test]
async fn stale_cached_payload_waits_for_a_fresh_one() {
    let mut server = MockLazer::start().await;
    let source = source_for(&server, Duration::from_millis(200));

    let probe = source.clone();
    let fetch = tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await });
    let mut connection = accept_and_serve(&mut server).await;
    timeout(Duration::from_secs(5), fetch)
        .await
        .expect("first fetch should not time out")
        .expect("fetch task should not panic")
        .expect("first fetch should return the payload");

    tokio::time::sleep(Duration::from_millis(400)).await;

    let probe = source.clone();
    let mut fetch = tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await });

    assert!(
        timeout(Duration::from_millis(200), &mut fetch)
            .await
            .is_err(),
        "a stale cached payload must not be returned, and must not fail the fetch"
    );

    send(&mut connection, &fixture_response(SubscriptionId(1))).await;

    let payload = timeout(Duration::from_secs(5), fetch)
        .await
        .expect("fetch should return once a fresh payload arrives")
        .expect("fetch task should not panic")
        .expect("fetch should return the payload");

    assert_eq!(payload, fixture_bytes());
}

/// A silent websocket must trip the idle timeout even while fetches keep arriving.
/// Every fetch messages the task, including one for an already-covered subscription,
/// so an idle timer rebuilt per `select!` iteration would be reset forever and the
/// dead socket would never be detected.
#[tokio::test]
async fn silent_stream_reconnects_despite_fetch_traffic() {
    let mut server = MockLazer::start().await;
    let source = source_with_idle_timeout(
        &server,
        Duration::from_millis(100),
        Duration::from_millis(500),
    );

    let probe = source.clone();
    let fetch = tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await });
    // Held open, then silent forever: the server never sends another frame.
    let _connection = accept_and_serve(&mut server).await;
    timeout(Duration::from_secs(5), fetch)
        .await
        .expect("first fetch should not time out")
        .expect("fetch task should not panic")
        .expect("first fetch should return the payload");

    let poller = tokio::spawn(async move {
        for _ in 0..40_u32 {
            let probe = source.clone();
            tokio::spawn(async move { probe.fetch_payload(&[FIXTURE_FEED]).await });
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

/// A requester that timed out or was cancelled has dropped its receiver before the
/// task handles its request. Registering it anyway would strand a subscription that
/// nothing waits on — holding the connection open forever — and, at capacity, evict a
/// live one to make room for it.
#[test]
fn an_abandoned_request_registers_nothing() {
    let mut task = StreamTask::new(LazerSourceConfig::for_mock_server(
        "ws://127.0.0.1:1/v1/stream".parse().expect("valid url"),
        Duration::from_secs(5),
    ));
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
