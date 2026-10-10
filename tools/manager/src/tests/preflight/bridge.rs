use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

pub(super) const OLD_TIMESTAMP_MS: u64 = 1_770_985_144_000;
pub(super) const NEW_TIMESTAMP_MS: u64 = 1_771_336_150_000;

// Exact signed ETH/BTC packages from contract/redstone-adapter/tests/test.rs:
// case::stellar and case::js_sdk, respectively. Their signatures and package
// timestamps are retained; neither the fixture nor the manager signs new data.
const OLD_HEX: &str = include_str!("../../../fixtures/preflight/old.hex");
const NEW_HEX: &str = include_str!("../../../fixtures/preflight/new.hex");
const NODE_SCRIPT: &str = include_str!("../../../fixtures/preflight/node.js");

pub(super) fn old_payload() -> Vec<u8> {
    hex::decode(OLD_HEX.trim()).expect("checked-in stellar signed payload is valid hex")
}

pub(super) fn new_payload() -> Vec<u8> {
    hex::decode(NEW_HEX.trim()).expect("checked-in js_sdk signed payload is valid hex")
}

pub(super) struct BridgeFixture {
    directory: PathBuf,
    node: PathBuf,
}

impl BridgeFixture {
    pub(super) fn new() -> Result<Self> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "tmplrmgr-preflight-{}-{timestamp}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory).context("creating preflight bridge fixture directory")?;
        let fixture = Self {
            node: directory.join("node.js"),
            directory,
        };
        fs::set_permissions(&fixture.directory, fs::Permissions::from_mode(0o700))?;
        fs::write(&fixture.node, NODE_SCRIPT)?;
        fs::set_permissions(&fixture.node, fs::Permissions::from_mode(0o700))?;
        fs::write(fixture.directory.join("new.hex"), NEW_HEX)?;
        fs::write(fixture.directory.join("requests.jsonl"), "")?;
        fs::write(fixture.directory.join("pids.jsonl"), "")?;
        fixture.set_failure(false)?;
        Ok(fixture)
    }

    pub(super) fn node_path(&self) -> &Path {
        &self.node
    }

    pub(super) fn set_failure(&self, failure: bool) -> Result<()> {
        // Atomic replacement prevents a live bridge reading a partial JSON file.
        let pending = self.directory.join("config.pending");
        fs::write(
            &pending,
            serde_json::to_vec(&serde_json::json!({ "failure": failure }))?,
        )?;
        fs::rename(pending, self.directory.join("config.json"))?;
        Ok(())
    }

    pub(super) fn request_count(&self) -> Result<usize> {
        Ok(fs::read_to_string(self.directory.join("requests.jsonl"))?
            .lines()
            .count())
    }

    pub(super) fn startup_count(&self) -> Result<usize> {
        Ok(fs::read_to_string(self.directory.join("pids.jsonl"))?
            .lines()
            .count())
    }
}

#[derive(serde::Deserialize)]
struct ProcessRecord {
    pid: u32,
    start_time: String,
}

impl BridgeFixture {
    fn owns_process(&self, record: &ProcessRecord) -> bool {
        // Reject dangerous PIDs and PID reuse. Both the process birth time and
        // exact executable-script argument must match before sending a signal.
        if record.pid <= 1 || record.pid == std::process::id() {
            return false;
        }
        let proc = PathBuf::from(format!("/proc/{}", record.pid));
        let Ok(stat) = fs::read_to_string(proc.join("stat")) else {
            return false;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            return false;
        };
        if fields.split_whitespace().nth(19) != Some(record.start_time.as_str()) {
            return false;
        }
        let Ok(arguments) = fs::read(proc.join("cmdline")) else {
            return false;
        };
        arguments
            .split(|byte| *byte == 0)
            .any(|argument| argument == self.node.as_os_str().as_encoded_bytes())
    }
}

impl Drop for BridgeFixture {
    fn drop(&mut self) {
        if let Ok(records) = fs::read_to_string(self.directory.join("pids.jsonl")) {
            let records: Vec<ProcessRecord> = records
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            for record in &records {
                if self.owns_process(record) {
                    // The production bridge owns/reaps the child; we only stop
                    // fixture children that outlive its context. No shell.
                    let _ = Command::new("kill")
                        .args(["-KILL", "--", &record.pid.to_string()])
                        .status();
                }
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while records.iter().any(|record| self.owns_process(record))
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}
