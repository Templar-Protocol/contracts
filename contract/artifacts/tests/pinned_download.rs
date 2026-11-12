#![cfg(feature = "fetch")]

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use rstest::rstest;
use templar_contract_artifacts::{fetch, sha256_hex};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

const WASM: &[u8] = b"\0asm\x01\0\0\0";
const PREVIOUS: &[u8] = b"\0asm\x01\0\0\0\0\x04\x03old";

struct Output(PathBuf);

impl Output {
    fn new() -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "templar-pin-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&directory)?;
        Ok(Self(directory.join("fixture.wasm")))
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    Verified,
    WrongLength,
    WrongHash,
    NotWasm,
    MissingAsset,
}

async fn serve_once(
    body: Vec<u8>,
    status: u16,
) -> std::io::Result<(String, tokio::task::JoinHandle<std::io::Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/fixture.wasm", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let (connection, _) = listener.accept().await?;
        let mut request = BufReader::new(connection);
        let mut line = String::new();
        loop {
            line.clear();
            if request.read_line(&mut line).await? == 0 || line == "\r\n" {
                break;
            }
        }
        let mut connection = request.into_inner();
        let header = format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len(),
        );
        connection.write_all(header.as_bytes()).await?;
        connection.write_all(&body).await?;
        connection.shutdown().await
    });
    Ok((url, server))
}

#[rstest]
#[case::verified(Outcome::Verified)]
#[case::wrong_length(Outcome::WrongLength)]
#[case::wrong_hash(Outcome::WrongHash)]
#[case::not_wasm(Outcome::NotWasm)]
#[case::missing_asset(Outcome::MissingAsset)]
#[tokio::test]
async fn only_verified_wasm_replaces_existing_output(#[case] outcome: Outcome) {
    let output = Output::new().unwrap();
    std::fs::write(&output.0, PREVIOUS).unwrap();
    let body = if matches!(outcome, Outcome::NotWasm) {
        b"not wasm".to_vec()
    } else {
        WASM.to_vec()
    };
    let length = body.len() + usize::from(matches!(outcome, Outcome::WrongLength));
    let sha256 = if matches!(outcome, Outcome::WrongHash) {
        "0".repeat(64)
    } else {
        sha256_hex(&body)
    };
    let status = if matches!(outcome, Outcome::MissingAsset) {
        404
    } else {
        200
    };
    let (url, server) = serve_once(body.clone(), status).await.unwrap();
    let result = fetch::download_pinned(&url, length, &sha256, &output.0).await;
    // A terminal-status response may make the client close before reading its body.
    // The assertions below still require the exact download outcome.
    let _ = server.await.expect("HTTP fixture finishes");

    match outcome {
        Outcome::Verified => {
            assert_eq!(result.unwrap(), body);
            assert_eq!(std::fs::read(&output.0).unwrap(), body);
        }
        failure => {
            let error = result.expect_err("unverified response must fail");
            assert!(match failure {
                Outcome::WrongLength =>
                    matches!(error, fetch::FetchError::PinnedLengthMismatch { .. }),
                Outcome::WrongHash => matches!(error, fetch::FetchError::PinnedHashMismatch { .. }),
                Outcome::NotWasm => matches!(error, fetch::FetchError::InvalidWasm { .. }),
                Outcome::MissingAsset =>
                    matches!(error, fetch::FetchError::Status { status: 404, .. }),
                Outcome::Verified => unreachable!(),
            });
            assert_eq!(std::fs::read(&output.0).unwrap(), PREVIOUS);
        }
    }
}
