//! LayerZero checked native adapter: desired/effective route comparison,
//! typed Type-3 security and executor config, and directional containment
//! plans. Pure decisions over typed inputs; no live chain mutation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::domain::{DesiredRouteV1, Direction, OperationV1, RouteStateV1, Vm};
use crate::error::{Error, Result};

/// Stellar outbound containment sets the send library to zero.
pub const STELLAR_ZERO_LIBRARY: &str = "0";

/// One drifted route field between desired and effective state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteField {
    /// Free-form route config key.
    Config(String),
    /// Peer contract for a remote endpoint id.
    Peer(u32),
}

/// Typed drift entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteDriftV1 {
    pub field: RouteField,
    pub desired: String,
    pub effective: String,
}

/// Result of comparing a desired route against recorded effective state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteComparisonV1 {
    pub route_id: String,
    pub drift: Vec<RouteDriftV1>,
    pub converged: bool,
}

fn config_string(config: &BTreeMap<String, serde_json::Value>, key: &str) -> String {
    config
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Compares the desired route against the recorded effective route state.
/// Every config key and peer contract present on either side must agree;
/// mismatches and missing sides are drift.
pub fn compare_routes(desired: &DesiredRouteV1, state: &RouteStateV1) -> Result<RouteComparisonV1> {
    if state.route_id != desired.route_id {
        return Err(Error::Conflict(format!(
            "route state {} does not match desired route {}",
            state.route_id, desired.route_id
        )));
    }
    let mut keys: BTreeSet<&String> = desired.config.keys().collect();
    keys.extend(state.effective_config.keys());
    let mut drift = Vec::new();
    for key in keys {
        let desired_value = config_string(&desired.config, key);
        let effective_value = config_string(&state.effective_config, key);
        if desired_value != effective_value {
            drift.push(RouteDriftV1 {
                field: RouteField::Config(key.clone()),
                desired: desired_value,
                effective: effective_value,
            });
        }
    }
    let peers = [
        (
            desired.identity.stellar_eid,
            desired.identity.stellar_endpoint.as_str(),
        ),
        (
            desired.identity.evm_eid,
            desired.identity.evm_endpoint.as_str(),
        ),
    ];
    for (eid, desired_peer) in peers {
        let effective_peer = state
            .contracts
            .get(format!("peer:{eid}").as_str())
            .map(String::as_str)
            .unwrap_or_default();
        if effective_peer != desired_peer {
            drift.push(RouteDriftV1 {
                field: RouteField::Peer(eid),
                desired: desired_peer.to_string(),
                effective: effective_peer.to_string(),
            });
        }
    }
    Ok(RouteComparisonV1 {
        route_id: desired.route_id.clone(),
        converged: drift.is_empty(),
        drift,
    })
}

/// Checked native adapter trait for route comparison.
pub trait RouteComparisonAdapter {
    fn compare(&self, desired: &DesiredRouteV1, state: &RouteStateV1) -> Result<RouteComparisonV1>;
}

/// Checked adapter implementation of [RouteComparisonAdapter].
#[derive(Debug, Default)]
pub struct CheckedRouteComparison;

impl RouteComparisonAdapter for CheckedRouteComparison {
    fn compare(&self, desired: &DesiredRouteV1, state: &RouteStateV1) -> Result<RouteComparisonV1> {
        compare_routes(desired, state)
    }
}

/// Type-3 ULN security config: required and optional DVNs with threshold and
/// block confirmations.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UlnConfigType3V1 {
    pub required_dvns: Vec<String>,
    pub optional_dvns: Vec<String>,
    pub optional_threshold: u8,
    pub confirmations: u32,
}

impl UlnConfigType3V1 {
    /// Checks structural invariants of the security config.
    pub fn validate(&self) -> Result<()> {
        let check = |dvns: &[String]| {
            if dvns.iter().any(|dvn| dvn.trim().is_empty()) {
                return Err(Error::InvalidInput(
                    "dvn identifiers must not be empty".into(),
                ));
            }
            let unique = dvns.iter().collect::<BTreeSet<_>>();
            if unique.len() != dvns.len() {
                return Err(Error::InvalidInput("dvn identifiers must be unique".into()));
            }
            Ok(())
        };
        check(&self.required_dvns)?;
        check(&self.optional_dvns)?;
        if usize::from(self.optional_threshold) > self.optional_dvns.len() {
            return Err(Error::InvalidInput(
                "optional threshold exceeds optional dvn count".into(),
            ));
        }
        if self.confirmations == 0 {
            return Err(Error::InvalidInput(
                "confirmations must be at least 1".into(),
            ));
        }
        Ok(())
    }

    /// Deterministic canonical hash binding the config into operations.
    pub fn config_sha256(&self) -> Result<String> {
        self.validate()?;
        crate::canonical_sha256(self)
    }
}

/// Type-3 executor config.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutorConfigType3V1 {
    /// Explicit executor address; empty means the default executor.
    pub executor: String,
}

impl ExecutorConfigType3V1 {
    /// Checks structural invariants of the executor config.
    pub fn validate(&self) -> Result<()> {
        Ok(())
    }

    /// Deterministic canonical hash binding the config into operations.
    pub fn config_sha256(&self) -> Result<String> {
        self.validate()?;
        crate::canonical_sha256(self)
    }
}

/// Builds the typed ULN operation for a config.
pub fn set_uln_operation(
    vm: Vm,
    remote_eid: u32,
    config: &UlnConfigType3V1,
) -> Result<OperationV1> {
    let config_sha256 = config.config_sha256()?;
    Ok(match vm {
        Vm::Stellar => OperationV1::SetStellarUlnConfig {
            remote_eid,
            config_sha256,
        },
        Vm::Evm => OperationV1::SetEvmUlnConfig {
            remote_eid,
            config_sha256,
        },
    })
}

/// Builds the typed executor operation for a config.
pub fn set_executor_operation(
    vm: Vm,
    remote_eid: u32,
    config: &ExecutorConfigType3V1,
) -> Result<OperationV1> {
    let config_sha256 = config.config_sha256()?;
    Ok(match vm {
        Vm::Stellar => OperationV1::SetStellarExecutorConfig {
            remote_eid,
            config_sha256,
        },
        Vm::Evm => OperationV1::SetEvmExecutorConfig {
            remote_eid,
            config_sha256,
        },
    })
}

/// Recorded send/receive library state for one direction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LibrarySnapshotV1 {
    pub vm: Vm,
    pub direction: Direction,
    pub send_library: String,
    pub receive_library: String,
}

/// Directional containment plan. The send library is replaced while the
/// receive library is preserved verbatim.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContainmentPlanV1 {
    pub vm: Vm,
    pub direction: Direction,
    pub contained_send_library: String,
    pub preserved_receive_library: String,
    pub snapshot_sha256: String,
    pub snapshot: LibrarySnapshotV1,
}

/// Checked native adapter trait for containment planning and restore.
pub trait LayerZeroContainmentAdapter {
    /// Stellar outbound containment: send library set to zero, receive
    /// library preserved.
    fn plan_stellar_outbound_zero(&self, snapshot: &LibrarySnapshotV1)
        -> Result<ContainmentPlanV1>;
    /// EVM outbound containment: send library set to the blocked library,
    /// receive library preserved.
    fn plan_evm_blocked_library(
        &self,
        snapshot: &LibrarySnapshotV1,
        blocked_library: &str,
    ) -> Result<ContainmentPlanV1>;
    /// Restore operation referencing the preserved snapshot.
    fn restore_operation(&self, plan: &ContainmentPlanV1) -> Result<OperationV1>;
}

fn containment_plan(
    snapshot: &LibrarySnapshotV1,
    expected_vm: Vm,
    contained_send_library: &str,
) -> Result<ContainmentPlanV1> {
    if snapshot.vm != expected_vm {
        return Err(Error::InvalidInput(format!(
            "containment snapshot vm {:?} does not match expected {expected_vm:?}",
            snapshot.vm
        )));
    }
    if snapshot.receive_library.trim().is_empty() {
        return Err(Error::InvalidInput(
            "containment requires a recorded receive library to preserve".into(),
        ));
    }
    if contained_send_library.trim().is_empty() {
        return Err(Error::InvalidInput(
            "contained send library must not be empty".into(),
        ));
    }
    let snapshot_sha256 = crate::canonical_sha256(snapshot)?;
    Ok(ContainmentPlanV1 {
        vm: snapshot.vm,
        direction: snapshot.direction,
        contained_send_library: contained_send_library.to_string(),
        preserved_receive_library: snapshot.receive_library.clone(),
        snapshot_sha256,
        snapshot: snapshot.clone(),
    })
}

/// Checked adapter implementation of [LayerZeroContainmentAdapter].
#[derive(Debug, Default)]
pub struct CheckedLayerZeroContainment;

impl LayerZeroContainmentAdapter for CheckedLayerZeroContainment {
    fn plan_stellar_outbound_zero(
        &self,
        snapshot: &LibrarySnapshotV1,
    ) -> Result<ContainmentPlanV1> {
        containment_plan(snapshot, Vm::Stellar, STELLAR_ZERO_LIBRARY)
    }

    fn plan_evm_blocked_library(
        &self,
        snapshot: &LibrarySnapshotV1,
        blocked_library: &str,
    ) -> Result<ContainmentPlanV1> {
        containment_plan(snapshot, Vm::Evm, blocked_library)
    }

    fn restore_operation(&self, plan: &ContainmentPlanV1) -> Result<OperationV1> {
        Ok(OperationV1::RestoreOutbound {
            snapshot_sha256: plan.snapshot_sha256.clone(),
        })
    }
}

/// The `ContainOutbound` operation a plan drives.
pub fn contain_operation(plan: &ContainmentPlanV1) -> OperationV1 {
    OperationV1::ContainOutbound {
        direction: plan.direction,
    }
}

/// Reports recorded containment state without mutating either chain.
pub fn containment_status(state: &std::path::Path) -> Result<crate::output::CommandData> {
    let state = crate::state::RouteStore::open(state)?.load_state()?;
    Ok(crate::output::CommandData {
        result: serde_json::json!({
            "stellar": state.effective_config.get("containment:stellar"),
            "evm": state.effective_config.get("containment:evm")
        }),
        artifact: None,
    })
}
