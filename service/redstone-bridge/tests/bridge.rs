#![allow(clippy::unwrap_used)]

use std::{env, path::PathBuf};

#[test]
fn js() {
    let crate_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bridge_dir = crate_root.join("js");

    if env::var("CI").is_ok_and(|v| !v.is_empty()) {
        let status = std::process::Command::new("npm")
            .arg("ci")
            .current_dir(&bridge_dir)
            .status()
            .expect("failed to run npm ci");
        assert!(status.success(), "npm ci failed");
    } else {
        // Verify node_modules exists; prompt developer to install if missing
        assert!(
            bridge_dir.join("node_modules").exists(),
            "node_modules not found in {}. Run `npm install` first.",
            bridge_dir.display(),
        );
    }

    // check exit code to ensure tests pass
    let status = std::process::Command::new("npm")
        .arg("test")
        .current_dir(&bridge_dir)
        .status()
        .expect("failed to run npm test");
    assert!(status.success(), "npm test failed");
}

// ---------------------------------------------------------------------------
// Live bridge tests (require Node.js + internet).
// Excluded from CI via the nextest `ci` profile filter.
// ---------------------------------------------------------------------------

use std::path::Path;

use templar_redstone_bridge::{Bridge, BridgeError};
use tokio::sync::watch;

#[tokio::test]
async fn requires_network_single_feed() {
    let (kill, _) = watch::channel(());
    let bridge = Bridge::new(Path::new("node"), kill).unwrap();

    let payload = bridge.fetch(vec!["BTC".into()]).await.unwrap();
    assert!(!payload.is_empty());
}

#[tokio::test]
async fn requires_network_multiple_feeds() {
    let (kill, _) = watch::channel(());
    let bridge = Bridge::new(Path::new("node"), kill).unwrap();

    let payload = bridge
        .fetch(vec!["BTC".into(), "ETH".into()])
        .await
        .unwrap();
    assert!(!payload.is_empty());
}

#[tokio::test]
async fn requires_network_sequential_requests() {
    let (kill, _) = watch::channel(());
    let bridge = Bridge::new(Path::new("node"), kill).unwrap();

    let first = bridge.fetch(vec!["BTC".into()]).await.unwrap();
    let second = bridge.fetch(vec!["ETH".into()]).await.unwrap();

    assert!(!first.is_empty());
    assert!(!second.is_empty());
    // Different feeds should produce different payloads.
    assert_ne!(first, second);
}

#[tokio::test]
async fn exits_when_killed() {
    let (kill, _) = watch::channel(());
    let bridge = Bridge::new(Path::new("node"), kill.clone()).unwrap();

    kill.send(()).unwrap();

    let error = bridge.fetch(vec!["BTC".into()]).await.unwrap_err();
    assert!(
        matches!(error, BridgeError::Recv(_)),
        "expected Recv error, got: {error:?}"
    );
}

#[tokio::test]
async fn invalid_node_path() {
    let (kill, _) = watch::channel(());
    let error = Bridge::new(Path::new("/nonexistent/node"), kill).unwrap_err();
    assert!(
        matches!(error, BridgeError::StartBridge(_)),
        "expected StartBridge error, got: {error:?}"
    );
}
