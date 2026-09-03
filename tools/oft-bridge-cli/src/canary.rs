//! Canary and packet message monotonic state machine with the
//! additional-obligation registry. Pure state transitions; no live mutation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Monotonic canary stages for one bridge message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanaryStage {
    /// Message armed, not yet sent.
    Armed,
    /// Message submitted to the source chain.
    Dispatched,
    /// Message observed delivered on the destination chain.
    Delivered,
    /// Destination application acknowledged the message.
    Acknowledged,
    /// Value settled; the message lifecycle is complete.
    Settled,
}

impl CanaryStage {
    /// Monotonic rank used to order transitions.
    pub fn rank(self) -> u8 {
        match self {
            Self::Armed => 0,
            Self::Dispatched => 1,
            Self::Delivered => 2,
            Self::Acknowledged => 3,
            Self::Settled => 4,
        }
    }

    /// Parses the canonical snake_case label at an input boundary.
    pub fn parse(label: &str) -> Result<Self> {
        match label.trim() {
            "armed" => Ok(Self::Armed),
            "dispatched" => Ok(Self::Dispatched),
            "delivered" => Ok(Self::Delivered),
            "acknowledged" => Ok(Self::Acknowledged),
            "settled" => Ok(Self::Settled),
            other => Err(Error::InvalidInput(format!("unknown canary stage {other}"))),
        }
    }

    /// Canonical label of the stage.
    pub fn label(self) -> &'static str {
        match self {
            Self::Armed => "armed",
            Self::Dispatched => "dispatched",
            Self::Delivered => "delivered",
            Self::Acknowledged => "acknowledged",
            Self::Settled => "settled",
        }
    }
}

/// Monotonic state of one canary or packet message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanaryMessageV1 {
    guid: String,
    stage: CanaryStage,
    ledger_sequence: Option<u64>,
    obligations: BTreeMap<String, String>,
}

impl CanaryMessageV1 {
    /// Arms a new message; the only entry point into the machine.
    pub fn arm(guid: &str) -> Result<Self> {
        if guid.trim().is_empty() {
            return Err(Error::InvalidInput("message guid must not be empty".into()));
        }
        Ok(Self {
            guid: guid.to_string(),
            stage: CanaryStage::Armed,
            ledger_sequence: None,
            obligations: BTreeMap::new(),
        })
    }

    /// Message guid.
    pub fn guid(&self) -> &str {
        &self.guid
    }

    /// Current stage.
    pub fn stage(&self) -> CanaryStage {
        self.stage
    }

    /// Highest observed ledger or block sequence, when recorded.
    pub fn ledger_sequence(&self) -> Option<u64> {
        self.ledger_sequence
    }

    /// Recorded obligations.
    pub fn obligations(&self) -> &BTreeMap<String, String> {
        &self.obligations
    }

    /// Whether the message is in the recovery-eligible window: moving but
    /// not yet settled.
    pub fn can_recover(&self) -> bool {
        matches!(
            self.stage,
            CanaryStage::Dispatched | CanaryStage::Delivered | CanaryStage::Acknowledged
        )
    }

    /// Advances the stage. Transitions are strictly forward: equal or
    /// backwards transitions are conflicts.
    pub fn advance(&mut self, to: CanaryStage, ledger_sequence: Option<u64>) -> Result<()> {
        if to.rank() <= self.stage.rank() {
            return Err(Error::Conflict(format!(
                "canary message {} cannot move from {} backwards or sideways to {}",
                self.guid,
                self.stage.label(),
                to.label()
            )));
        }
        if let Some(sequence) = ledger_sequence {
            if let Some(previous) = self.ledger_sequence {
                if sequence < previous {
                    return Err(Error::Conflict(format!(
                        "canary message {} observed sequence {sequence} below recorded {previous}",
                        self.guid
                    )));
                }
            }
            self.ledger_sequence = Some(sequence);
        }
        self.stage = to;
        Ok(())
    }

    /// Records an additional obligation. Duplicate keys are refused unless
    /// `allow_override` is set.
    pub fn set_obligation(&mut self, key: &str, value: &str, allow_override: bool) -> Result<()> {
        if key.trim().is_empty() {
            return Err(Error::InvalidInput(
                "obligation key must not be empty".into(),
            ));
        }
        if value.trim().is_empty() {
            return Err(Error::InvalidInput(
                "obligation value must not be empty".into(),
            ));
        }
        if !allow_override && self.obligations.contains_key(key) {
            return Err(Error::Conflict(format!(
                "obligation {key} already recorded for message {}",
                self.guid
            )));
        }
        self.obligations.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Reads a recorded obligation.
    pub fn obligation(&self, key: &str) -> Option<&str> {
        self.obligations.get(key).map(String::as_str)
    }
}

use crate::domain::Direction;
use crate::output::CommandData;
use crate::state::RouteStore;
use std::path::Path;

fn route_environment(state_path: &Path) -> Result<crate::domain::RouteStateV1> {
    let state = RouteStore::open(state_path)?.load_state()?;
    crate::environment::require_testnet(&state.identity)?;
    Ok(state)
}

/// `leg quote`: writes a non-authoritative leg intent; no nonce reservation.
pub fn quote(
    state_path: &Path,
    direction: Direction,
    amount_raw: u128,
    to: &str,
    out: &Path,
) -> Result<CommandData> {
    let state = route_environment(state_path)?;
    if amount_raw == 0 {
        return Err(Error::InvalidInput(
            "amount_raw must be greater than zero".into(),
        ));
    }
    if to.trim().is_empty() {
        return Err(Error::InvalidInput("destination must not be empty".into()));
    }
    let intent = serde_json::json!({
        "schema_name": "leg_intent",
        "schema_version": 1,
        "route_id": state.route_id,
        "direction": direction,
        "amount_raw": amount_raw.to_string(),
        "to": to
    });
    crate::state::write_create_new_json(out, &intent)?;
    Ok(CommandData {
        result: intent,
        artifact: None,
    })
}

/// `leg send`: broadcasting requires a qualified live adapter; fail-closed.
pub fn send(
    state_path: &Path,
    intent: &Path,
    _allow_additional_obligation: bool,
    _execute: bool,
    _proposal_out: Option<&Path>,
) -> Result<CommandData> {
    let _state = route_environment(state_path)?;
    let _intent: serde_json::Value = crate::state::read_json(intent)?;
    Err(Error::Chain(
        "leg send requires a qualified live adapter; quote artifacts are non-authoritative".into(),
    ))
}

/// `message watch`: read-only observation requires a live Scan client.
pub fn watch(state_path: &Path, guid: &str) -> Result<CommandData> {
    let _state = route_environment(state_path)?;
    if guid.trim().is_empty() {
        return Err(Error::InvalidInput("guid must not be empty".into()));
    }
    Err(Error::Chain(
        "message watch requires a qualified live Scan client".into(),
    ))
}

/// `message recover`: capability-gated packet recovery.
pub fn recover(
    state_path: &Path,
    guid: &str,
    _execute: bool,
    _proposal_out: Option<&Path>,
) -> Result<CommandData> {
    let _state = route_environment(state_path)?;
    if guid.trim().is_empty() {
        return Err(Error::InvalidInput("guid must not be empty".into()));
    }
    Err(Error::Chain(
        "message recovery requires qualified packet evidence and a live adapter".into(),
    ))
}

/// `evidence import`: validates an evidence bundle against route binding.
pub fn import_evidence(state_path: &Path, bundle: &Path, write: bool) -> Result<CommandData> {
    let state = route_environment(state_path)?;
    let bundle_value: serde_json::Value = crate::state::read_json(bundle)?;
    let bundle_route = bundle_value
        .get("route_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::InvalidInput("evidence bundle must carry route_id".into()))?;
    if bundle_route != state.route_id {
        return Err(Error::Conflict(
            "evidence bundle does not bind to this route".into(),
        ));
    }
    if !write {
        return Ok(CommandData {
            result: serde_json::json!({"preview": true}),
            artifact: None,
        });
    }
    Err(Error::Chain(
        "durable evidence import requires a qualified custody adapter".into(),
    ))
}
