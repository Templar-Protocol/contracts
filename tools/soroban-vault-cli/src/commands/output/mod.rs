//! Stable machine response models and output routing.

mod error;
mod render;

use serde::Serialize;

use crate::stellar::parse_labeled_tx_hashes;

pub use error::{print_error, print_parse_error};
pub(super) use render::print_response;

#[cfg(test)]
pub(in crate::commands) use error::{OutputEnvelope, ParseErrorEnvelope};

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Response {
    Message { message: String },
    Command { stdout: String, stderr: String },
    Status(StatusResponse),
    Env(Vec<(String, String)>),
    ExtendTtl(ExtendTtlResponse),
    Reconcile(ReconcileResponse),
    Doctor(DoctorResponse),
    Plan(PlanResponse),
    GovernanceQueue(GovernanceQueueResponse),
    GovernanceExplain(GovernanceProposalView),
    GovernanceAcceptReady(GovernanceAcceptReadyResponse),
}

impl Response {
    pub(super) fn message(message: String) -> Self {
        Self::Message { message }
    }

    pub(super) const fn kind(&self) -> &'static str {
        match self {
            Self::Message { .. } => "message",
            Self::Command { .. } => "command",
            Self::Status(_) => "status",
            Self::Env(_) => "env",
            Self::ExtendTtl(_) => "extend_ttl",
            Self::Reconcile(_) => "reconcile",
            Self::Doctor(_) => "doctor",
            Self::Plan(_) => "plan",
            Self::GovernanceQueue(_) => "governance_queue",
            Self::GovernanceExplain(_) => "governance_explain",
            Self::GovernanceAcceptReady(_) => "governance_accept_ready",
        }
    }

    pub(super) fn warnings(&self) -> Vec<String> {
        match self {
            Self::Plan(plan) => plan.warnings.clone(),
            Self::GovernanceQueue(queue) => queue.warnings.clone(),
            Self::GovernanceAcceptReady(result) => result.skipped.clone(),
            Self::Reconcile(result) => result
                .components
                .iter()
                .flat_map(|component| component.warnings.clone())
                .collect(),
            Self::Doctor(result) => result
                .checks
                .iter()
                .filter(|check| check.status == DoctorStatus::Warn)
                .map(|check| format!("{}: {}", check.name, check.message))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn command_shapes(&self) -> Vec<String> {
        match self {
            Self::Plan(plan) => plan.stellar_commands.clone(),
            _ => Vec::new(),
        }
    }

    pub(super) fn tx_hashes(&self) -> Vec<String> {
        match self {
            Self::Command { stdout, stderr } => {
                let mut hashes = Vec::new();
                for hash in parse_labeled_tx_hashes(stdout)
                    .into_iter()
                    .chain(parse_labeled_tx_hashes(stderr))
                {
                    if !hashes.contains(&hash) {
                        hashes.push(hash);
                    }
                }
                hashes
            }
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct StatusResponse {
    pub(super) network: String,
    pub(super) vault: Option<String>,
    pub(super) share_token: Option<String>,
    pub(super) governance: Option<String>,
    pub(super) asset_token: Option<String>,
    pub(super) proxy_4626: Option<String>,
    pub(super) curator_proxy: Option<String>,
    pub(super) blend_adapters: Vec<BlendAdapterStatus>,
    pub(super) custodial_adapters: Vec<CustodialAdapterStatus>,
}

#[derive(Debug, Serialize)]
pub(super) struct BlendAdapterStatus {
    pub(super) key: String,
    pub(super) contract_id: String,
    pub(super) pool: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct CustodialAdapterStatus {
    pub(super) key: String,
    pub(super) contract_id: String,
    pub(super) custodian: Option<String>,
    pub(super) asset: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ExtendTtlResponse {
    pub(super) extended: Vec<String>,
    pub(super) skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ReconcileResponse {
    pub(super) safe_to_resume: bool,
    pub(super) drift_detected: bool,
    pub(super) components: Vec<ReconcileComponent>,
    pub(super) repair_actions: Vec<String>,
    pub(super) safe_next_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ReconcileComponent {
    pub(super) key: String,
    pub(super) contract_id: Option<String>,
    pub(super) manifest_recorded: bool,
    pub(super) manifest_initialized: bool,
    pub(super) recorded_wasm_hash: Option<String>,
    pub(super) chain_wasm_hash: Option<String>,
    pub(super) status: ReconcileStatus,
    pub(super) wiring: Vec<WiringCheck>,
    pub(super) warnings: Vec<String>,
    pub(super) repair_actions: Vec<String>,
}

impl ReconcileComponent {
    pub(super) const fn safe_to_resume(&self) -> bool {
        match self.status {
            ReconcileStatus::Initialized | ReconcileStatus::Deployed => true,
            ReconcileStatus::Missing => !self.manifest_recorded,
            ReconcileStatus::Unknown | ReconcileStatus::Mismatched => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReconcileStatus {
    Missing,
    Deployed,
    Initialized,
    Unknown,
    Mismatched,
}

impl ReconcileStatus {
    pub(super) const fn as_label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Deployed => "deployed",
            Self::Initialized => "initialized",
            Self::Unknown => "unknown",
            Self::Mismatched => "mismatched",
        }
    }

    pub(super) const fn is_drift(self) -> bool {
        matches!(self, Self::Unknown | Self::Mismatched)
    }
}

#[derive(Debug, Serialize)]
pub(super) struct WiringCheck {
    pub(super) field: String,
    pub(super) expected: Option<String>,
    pub(super) observed: Option<String>,
    pub(super) status: WiringStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WiringStatus {
    Match,
    Mismatch,
    Unknown,
}

#[derive(Debug, Serialize)]
pub(super) struct DoctorResponse {
    pub(super) ok: bool,
    pub(super) checks: Vec<DoctorCheck>,
}

#[derive(Debug, Serialize)]
pub(super) struct PlanResponse {
    pub(super) scope: String,
    pub(super) network: String,
    pub(super) required_signers: Vec<String>,
    pub(super) contracts_to_reuse: Vec<PlanContract>,
    pub(super) contracts_to_deploy: Vec<PlanContract>,
    pub(super) wasm: Vec<PlanWasm>,
    pub(super) manifest_mutations: Vec<String>,
    pub(super) stellar_commands: Vec<String>,
    pub(super) warnings: Vec<String>,
}

impl PlanResponse {
    pub(super) fn new(scope: impl Into<String>, network: &str) -> Self {
        Self {
            scope: scope.into(),
            network: network.to_string(),
            required_signers: Vec::new(),
            contracts_to_reuse: Vec::new(),
            contracts_to_deploy: Vec::new(),
            wasm: Vec::new(),
            manifest_mutations: Vec::new(),
            stellar_commands: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct PlanContract {
    pub(super) key: String,
    pub(super) contract_id: Option<String>,
    pub(super) reason: String,
}

#[derive(Debug, Serialize)]
pub(super) struct PlanWasm {
    pub(super) key: String,
    pub(super) package: String,
    pub(super) path: String,
    pub(super) local_hash: Option<String>,
    pub(super) recorded_remote_hash: Option<String>,
    pub(super) action: String,
}

#[derive(Debug, Serialize)]
pub(super) struct GovernanceQueueResponse {
    pub(super) proposals: Vec<GovernanceProposalView>,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct GovernanceProposalView {
    pub(super) proposal_id: u64,
    pub(super) action: String,
    pub(super) valid_after_ns: Option<u64>,
    pub(super) ready: Option<bool>,
    pub(super) eta_seconds: Option<i64>,
    pub(super) raw: String,
}

#[derive(Debug, Serialize)]
pub(super) struct GovernanceAcceptReadyResponse {
    pub(super) accepted: Vec<u64>,
    pub(super) skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct DoctorCheck {
    pub(super) name: String,
    pub(super) status: DoctorStatus,
    pub(super) message: String,
}

impl DoctorCheck {
    pub(super) fn pass(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorStatus::Pass,
            message: message.into(),
        }
    }

    pub(super) fn warn(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorStatus::Warn,
            message: message.into(),
        }
    }

    pub(super) fn fail(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorStatus::Fail,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

impl DoctorStatus {
    pub(super) const fn as_label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}
