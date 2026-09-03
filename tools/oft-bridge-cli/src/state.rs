use std::collections::BTreeMap;
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead as _, BufReader, Write as _},
    path::{Component, Path, PathBuf},
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    canonical_sha256,
    domain::{
        ArtifactRefV1, DesiredRouteV1, MessageRecordV1, MessageStageV1, MessageStatusEventV1,
        RouteStateV1, SCHEMA_VERSION,
    },
    error::{Error, Result},
};

const STATE_FILE: &str = "route.json";
const OPERATIONS_FILE: &str = "operations.jsonl";
const MESSAGES_FILE: &str = "messages.jsonl";
const LOCK_FILE: &str = ".lock";

#[derive(Debug)]
pub struct RouteLock {
    path: PathBuf,
    _file: File,
}

impl RouteLock {
    pub fn acquire(state_dir: &Path) -> Result<Self> {
        let path = state_dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::Conflict(format!("route is busy: {}", state_dir.display()))
                } else {
                    Error::Io(error)
                }
            })?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for RouteLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogRecordV1<T> {
    pub schema_name: String,
    pub schema_version: u32,
    pub log_id: String,
    pub index: u64,
    pub previous_record_sha256: String,
    pub companion_artifact_sha256: Option<String>,
    pub canonical_payload_sha256: String,
    pub payload: T,
    pub record_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationEventV1 {
    pub operation_id: String,
    pub state: OperationState,
    pub detail: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Planned,
    ProposalPrepared,
    ProposalCommitted,
    Signed,
    SubmissionPending,
    Confirmed,
    Failed,
    Ambiguous,
}

#[derive(Debug)]
pub struct RouteStore {
    root: PathBuf,
}

impl RouteStore {
    pub fn create(root: &Path, desired: DesiredRouteV1) -> Result<(Self, RouteStateV1)> {
        validate_new_directory(root)?;
        fs::create_dir(root)?;
        set_directory_mode(root)?;

        let desired_sha256 = canonical_sha256(&desired)?;
        let state = RouteStateV1 {
            schema_name: "route_state".into(),
            schema_version: SCHEMA_VERSION,
            route_id: desired.route_id.clone(),
            desired_sha256,
            identity: desired.identity,
            asset: desired.asset,
            opening_custody: None,
            operations_log: PathBuf::from(OPERATIONS_FILE),
            messages_log: PathBuf::from(MESSAGES_FILE),
            lock_file: PathBuf::from(LOCK_FILE),
            contracts: BTreeMap::default(),
            effective_config: BTreeMap::default(),
        };
        let store = Self {
            root: root.to_path_buf(),
        };
        write_create_new_json(&store.root.join(STATE_FILE), &state)?;
        create_empty_file(&store.root.join(OPERATIONS_FILE))?;
        create_empty_file(&store.root.join(MESSAGES_FILE))?;
        sync_directory(&store.root)?;
        Ok((store, state))
    }

    pub fn open(root: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::InvalidInput(format!(
                "state path must be a real directory: {}",
                root.display()
            )));
        }
        let store = Self {
            root: root.to_path_buf(),
        };
        let state: RouteStateV1 = read_json(&store.root.join(STATE_FILE))?;
        if state.schema_name != "route_state" || state.schema_version != SCHEMA_VERSION {
            return Err(Error::InvalidInput("unsupported route state schema".into()));
        }
        validate_relative_file(&state.operations_log)?;
        validate_relative_file(&state.messages_log)?;
        validate_relative_file(&state.lock_file)?;
        store.verify_log::<OperationEventV1>(&state.operations_log, "operations")?;
        store.verify_log::<MessageRecordV1>(&state.messages_log, "messages")?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn lock(&self) -> Result<RouteLock> {
        RouteLock::acquire(&self.root)
    }

    pub fn load_state(&self) -> Result<RouteStateV1> {
        read_json(&self.root.join(STATE_FILE))
    }

    pub fn save_state(&self, state: &RouteStateV1) -> Result<()> {
        atomic_replace_json(&self.root.join(STATE_FILE), state)
    }

    pub fn append_operation(
        &self,
        payload: OperationEventV1,
        companion_artifact_sha256: Option<String>,
    ) -> Result<LogRecordV1<OperationEventV1>> {
        self.append_log(
            OPERATIONS_FILE,
            "operations",
            payload,
            companion_artifact_sha256,
        )
    }

    /// Appends a brand-new packet record. Identity is the append-only key
    /// `(source_eid, sender, nonce, guid)`; duplicates are conflicts.
    pub fn append_message(&self, mut record: MessageRecordV1) -> Result<()> {
        if record.status_events.is_empty() {
            return Err(Error::InvalidInput(
                "message record requires an initial status event".into(),
            ));
        }
        if record
            .status_events
            .iter()
            .any(|event| event.stage == MessageStageV1::Reobserved)
        {
            return Err(Error::InvalidInput(
                "initial message status must be an observed stage, not reobserved".into(),
            ));
        }
        if record.packet_sha256.trim().is_empty() || record.payload_sha256.trim().is_empty() {
            return Err(Error::InvalidInput(
                "message record requires packet and payload digests".into(),
            ));
        }
        if self.find_message(&record.identity())?.is_some() {
            return Err(Error::Conflict(format!(
                "message identity already recorded: guid {}",
                record.guid
            )));
        }
        record.schema_name = "message_record".into();
        record.schema_version = SCHEMA_VERSION;
        self.append_log(MESSAGES_FILE, "messages", record, None)?;
        Ok(())
    }

    /// Appends a status event to an existing packet. The ledger is
    /// append-only: history is never rewritten, the event lands as a new
    /// chained record carrying the updated snapshot. Immutable fields
    /// (packet/payload/config digests, amounts, coordinates) must not change.
    pub fn append_message_event(
        &self,
        identity: &(u32, String, String, String),
        event: MessageStatusEventV1,
    ) -> Result<()> {
        let mut latest = self
            .find_message(identity)?
            .ok_or_else(|| Error::Conflict("message identity not recorded".into()))?;
        let last = &latest
            .status_events
            .last()
            .ok_or_else(|| Error::Custody("recorded message lacks status events".into()))?
            .stage;
        if event.stage != MessageStageV1::Reobserved && &event.stage <= last {
            return Err(Error::InvalidInput(format!(
                "non-monotonic message status {:?} after {:?}",
                event.stage, last
            )));
        }
        latest.status_events.push(event);
        self.append_log(MESSAGES_FILE, "messages", latest, None)?;
        Ok(())
    }

    /// Loads every packet record folded to its latest snapshot, verifying the
    /// hash chain and identity uniqueness across history.
    pub fn load_messages(&self) -> Result<Vec<MessageRecordV1>> {
        let records = self.verify_log::<MessageRecordV1>(Path::new(MESSAGES_FILE), "messages")?;
        let mut folded: Vec<MessageRecordV1> = Vec::new();
        for record in records {
            let payload = record.payload;
            match folded
                .iter()
                .position(|existing| existing.identity() == payload.identity())
            {
                Some(index) => {
                    let existing = &folded[index];
                    if existing.packet_sha256 != payload.packet_sha256
                        || existing.payload_sha256 != payload.payload_sha256
                        || existing.config_snapshot_sha256 != payload.config_snapshot_sha256
                        || existing.amount_raw != payload.amount_raw
                        || existing.source_transaction != payload.source_transaction
                    {
                        return Err(Error::Custody(format!(
                            "immutable fields changed for guid {}",
                            payload.guid
                        )));
                    }
                    if payload.status_events.len() != existing.status_events.len() + 1 {
                        return Err(Error::Custody(format!(
                            "status history is not append-only for guid {}",
                            payload.guid
                        )));
                    }
                    folded[index] = payload;
                }
                None => folded.push(payload),
            }
        }
        Ok(folded)
    }

    fn find_message(
        &self,
        identity: &(u32, String, String, String),
    ) -> Result<Option<MessageRecordV1>> {
        Ok(self
            .load_messages()?
            .into_iter()
            .find(|record| &record.identity() == identity))
    }

    pub fn write_proposal<T: Serialize>(
        &self,
        relative_path: &Path,
        operation_id: &str,
        proposal: &T,
    ) -> Result<ArtifactRefV1> {
        validate_relative_file(relative_path)?;
        let final_path = self.root.join(relative_path);
        ensure_parent_under(&self.root, &final_path)?;
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
            set_directory_mode(parent)?;
        }
        if final_path.exists() {
            return Err(Error::Conflict(format!(
                "artifact path already exists: {}",
                final_path.display()
            )));
        }

        let bytes = serde_json_canonicalizer::to_vec(proposal)?;
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let temporary = final_path.with_extension("tmp");
        write_create_new_bytes(&temporary, &bytes)?;
        self.append_operation(
            OperationEventV1 {
                operation_id: operation_id.into(),
                state: OperationState::ProposalPrepared,
                detail: serde_json::json!({"path": relative_path, "sha256": sha256}),
            },
            Some(sha256.clone()),
        )?;
        fs::rename(&temporary, &final_path)?;
        if let Some(parent) = final_path.parent() {
            sync_directory(parent)?;
        }
        self.append_operation(
            OperationEventV1 {
                operation_id: operation_id.into(),
                state: OperationState::ProposalCommitted,
                detail: serde_json::json!({"path": relative_path, "sha256": sha256}),
            },
            Some(sha256.clone()),
        )?;
        Ok(ArtifactRefV1 {
            kind: "proposal".into(),
            path: relative_path.to_path_buf(),
            sha256,
            schema_version: SCHEMA_VERSION,
            authoritative: true,
        })
    }

    pub fn verify_log<T: DeserializeOwned + Serialize>(
        &self,
        relative_path: &Path,
        log_kind: &str,
    ) -> Result<Vec<LogRecordV1<T>>> {
        validate_relative_file(relative_path)?;
        let path = self.root.join(relative_path);
        let file = open_regular_read(&path)?;
        let mut records = Vec::new();
        let mut previous = genesis_hash(log_kind, &self.load_state()?.route_id);
        for (line_index, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let record: LogRecordV1<T> = serde_json::from_str(&line).map_err(|error| {
                Error::Custody(format!(
                    "invalid {log_kind} log record {}: {error}",
                    line_index + 1
                ))
            })?;
            if record.index != records.len() as u64 || record.previous_record_sha256 != previous {
                return Err(Error::Custody(format!(
                    "{log_kind} log chain mismatch at record {}",
                    record.index
                )));
            }
            let expected = record_hash(&record)?;
            if expected != record.record_sha256 {
                return Err(Error::Custody(format!(
                    "{log_kind} log digest mismatch at record {}",
                    record.index
                )));
            }
            previous.clone_from(&record.record_sha256);
            records.push(record);
        }
        Ok(records)
    }

    fn append_log<T: Clone + DeserializeOwned + Serialize>(
        &self,
        file_name: &str,
        log_kind: &str,
        payload: T,
        companion_artifact_sha256: Option<String>,
    ) -> Result<LogRecordV1<T>> {
        let existing = self.verify_log::<T>(Path::new(file_name), log_kind)?;
        let state = self.load_state()?;
        let index = existing.len() as u64;
        let previous_record_sha256 = existing.last().map_or_else(
            || genesis_hash(log_kind, &state.route_id),
            |record| record.record_sha256.clone(),
        );
        let canonical_payload_sha256 = canonical_sha256(&payload)?;
        let mut record = LogRecordV1 {
            schema_name: format!("{log_kind}_log_record"),
            schema_version: SCHEMA_VERSION,
            log_id: format!("{}:{log_kind}", state.route_id),
            index,
            previous_record_sha256,
            companion_artifact_sha256,
            canonical_payload_sha256,
            payload,
            record_sha256: String::new(),
        };
        record.record_sha256 = record_hash(&record)?;
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.root.join(file_name))?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(record)
    }
}

fn record_hash<T: Serialize>(record: &LogRecordV1<T>) -> Result<String> {
    #[derive(Serialize)]
    struct HashInput<'a, T> {
        schema_name: &'a str,
        schema_version: u32,
        log_id: &'a str,
        index: u64,
        previous_record_sha256: &'a str,
        companion_artifact_sha256: &'a Option<String>,
        canonical_payload_sha256: &'a str,
        payload: &'a T,
    }
    canonical_sha256(&HashInput {
        schema_name: &record.schema_name,
        schema_version: record.schema_version,
        log_id: &record.log_id,
        index: record.index,
        previous_record_sha256: &record.previous_record_sha256,
        companion_artifact_sha256: &record.companion_artifact_sha256,
        canonical_payload_sha256: &record.canonical_payload_sha256,
        payload: &record.payload,
    })
}

fn genesis_hash(log_kind: &str, route_id: &str) -> String {
    hex::encode(Sha256::digest(format!(
        "templar-oft-bridge-log-v1{route_id}{log_kind}"
    )))
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = open_regular_read(path)?;
    Ok(serde_json::from_reader(file)?)
}

pub fn write_create_new_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json_canonicalizer::to_vec(value)?;
    write_create_new_bytes(path, &bytes)
}

fn atomic_replace_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("state path has no parent".into()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::InvalidInput("state path has an invalid file name".into()))?;
    let temporary = parent.join(format!(".{file_name}.tmp"));
    if temporary.exists() {
        return Err(Error::Conflict(format!(
            "temporary state path exists: {}",
            temporary.display()
        )));
    }
    let bytes = serde_json_canonicalizer::to_vec(value)?;
    write_create_new_bytes(&temporary, &bytes)?;
    fs::rename(&temporary, path)?;
    sync_directory(parent)
}

fn write_create_new_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn create_empty_file(path: &Path) -> Result<()> {
    write_create_new_bytes(path, b"")
}

fn open_regular_read(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::InvalidInput(format!(
            "path must be a real file: {}",
            path.display()
        )));
    }
    Ok(File::open(path)?)
}

fn validate_new_directory(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(Error::Conflict(format!(
            "state directory already exists: {}",
            path.display()
        )));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(Error::InvalidInput("state path must not contain ..".into()));
    }
    Ok(())
}

fn validate_relative_file(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Error::InvalidInput(format!(
            "managed path must remain relative: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_parent_under(root: &Path, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("artifact path has no parent".into()))?;
    if !parent.starts_with(root) {
        return Err(Error::InvalidInput(
            "artifact escapes route directory".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn set_directory_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_directory_mode(_path: &Path) -> Result<()> {
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
