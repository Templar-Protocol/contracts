use std::collections::HashMap;

use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_primitives::{SU128, SU64};

/// Sputnik policy with its native JSON representation.
///
/// The transparent boundary supplies JSON Schema without replacing the pinned
/// contract's runtime serializer or policy types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Policy(#[schemars(with = "schema::Policy")] pub sputnikdao2::Policy);

/// Sputnik proposal, including its ID, in the contract's flattened JSON form.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ProposalOutput(pub ProposalRecord);

/// Flattened proposal identity and body, retaining the native caller field layout.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SputnikProposalOutput")]
pub struct ProposalRecord {
    pub id: u64,
    #[serde(flatten)]
    pub proposal: Proposal,
}

/// Read-side body shared by the canonical 2.3.1 DAO and the modern wire form.
///
/// Contract enums retain their native serializers. Audit history is absent on
/// the canonical DAO; `None` must not become a fabricated available empty log.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SputnikProposal")]
pub struct Proposal {
    pub proposer: AccountId,
    pub description: String,
    #[schemars(schema_with = "schema::proposal_kind")]
    pub kind: sputnikdao2::ProposalKind,
    #[schemars(with = "schema::ProposalStatus")]
    pub status: sputnikdao2::ProposalStatus,
    pub vote_counts: HashMap<String, [VoteCount; 3]>,
    #[schemars(with = "HashMap<AccountId, schema::Vote>")]
    pub votes: HashMap<AccountId, sputnikdao2::proposals::Vote>,
    pub submission_time: SU64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<schema::ProposalLog>>")]
    pub last_actions_log: Option<Vec<sputnikdao2::action_log::ProposalLog>>,
}

/// Preserve canonical numeric counts and modern decimal-string counts exactly.
///
/// Numeric counts are bounded by the JSON decoder's exact unsigned integer
/// range; larger counts remain supported in the modern `u128` string form.
/// Floats are rejected rather than rounded into an integer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "SputnikVoteCount")]
pub enum VoteCount {
    Number(u64),
    Decimal(SU128),
}

/// Get a Sputnik DAO's current policy and proposal bond.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "dao.getPolicy", output = Policy)]
pub struct GetPolicy {
    pub dao_id: AccountId,
}

/// Get a Sputnik DAO proposal by its exact ID.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "dao.getProposal", output = ProposalOutput)]
pub struct GetProposal {
    pub dao_id: AccountId,
    pub id: u64,
}

/// Submit a Sputnik DAO proposal with the current policy's exact bond.
///
/// Planning rejects known-ineligible proposers. Token-weight membership may be
/// unknown, and the contract remains authoritative for permission and bond checks.
/// Proposal submission is not approval or execution of the proposed actions.
#[derive(MethodSpec, Serialize, Deserialize, JsonSchema)]
#[method(write = "dao.addProposal")]
pub struct AddProposal {
    pub dao_id: AccountId,
    #[schemars(with = "schema::ProposalInput")]
    pub proposal: sputnikdao2::ProposalInput,
}

impl Clone for AddProposal {
    fn clone(&self) -> Self {
        Self {
            dao_id: self.dao_id.clone(),
            proposal: sputnikdao2::ProposalInput {
                description: self.proposal.description.clone(),
                kind: self.proposal.kind.clone(),
            },
        }
    }
}

// Schema-only counterparts for native policy, input, and enum serializers,
// including private ActionCall fields. The typed read body above owns its schema.
mod schema {
    #![allow(
        dead_code,
        reason = "these types describe pinned Sputnik JSON schemas only"
    )]

    use std::{
        collections::{HashMap, HashSet},
        num::NonZeroU64,
    };

    use near_account_id::AccountId;
    use schemars::JsonSchema;
    use templar_gateway_types::{Base64Bytes, CryptoHash, NearGas, NearToken};
    use templar_primitives::{SU128, SU64};

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikPolicy", deny_unknown_fields)]
    #[allow(
        clippy::struct_field_names,
        reason = "schema fields mirror Sputnik's fixed public wire names"
    )]
    pub struct Policy {
        roles: Vec<RolePermission>,
        default_vote_policy: VotePolicy,
        proposal_bond: NearToken,
        proposal_period: SU64,
        bounty_bond: NearToken,
        bounty_forgiveness_period: SU64,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikRolePermission", deny_unknown_fields)]
    struct RolePermission {
        name: String,
        kind: RoleKind,
        permissions: HashSet<String>,
        vote_policy: HashMap<String, VotePolicy>,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikRoleKind", deny_unknown_fields)]
    enum RoleKind {
        Everyone,
        Member(SU128),
        Group(HashSet<AccountId>),
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikVotePolicy", deny_unknown_fields)]
    struct VotePolicy {
        weight_kind: WeightKind,
        quorum: SU128,
        threshold: WeightOrRatio,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikWeightKind")]
    enum WeightKind {
        TokenWeight,
        RoleWeight,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikWeightOrRatio", untagged)]
    enum WeightOrRatio {
        Weight(SU128),
        Ratio(u64, NonZeroU64),
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikVersionedPolicy", untagged, deny_unknown_fields)]
    enum VersionedPolicy {
        Default(Vec<AccountId>),
        Current(Policy),
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikProposalInput")]
    pub struct ProposalInput {
        description: String,
        kind: ProposalKind,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikProposalKind", deny_unknown_fields)]
    enum ProposalKind {
        ChangeConfig {
            config: Config,
        },
        ChangePolicy {
            policy: VersionedPolicy,
        },
        AddMemberToRole {
            member_id: AccountId,
            role: String,
        },
        RemoveMemberFromRole {
            member_id: AccountId,
            role: String,
        },
        FunctionCall {
            receiver_id: AccountId,
            actions: Vec<ActionCall>,
        },
        UpgradeSelf {
            hash: CryptoHash,
        },
        UpgradeRemote {
            receiver_id: AccountId,
            method_name: String,
            hash: CryptoHash,
        },
        Transfer {
            token_id: String,
            receiver_id: AccountId,
            amount: SU128,
            msg: Option<String>,
        },
        SetStakingContract {
            staking_id: AccountId,
        },
        AddBounty {
            bounty: Bounty,
        },
        BountyDone {
            bounty_id: u64,
            receiver_id: AccountId,
        },
        Vote,
        FactoryInfoUpdate {
            factory_info: FactoryInfo,
        },
        ChangePolicyAddOrUpdateRole {
            role: RolePermission,
        },
        ChangePolicyRemoveRole {
            role: String,
        },
        ChangePolicyUpdateDefaultVotePolicy {
            vote_policy: VotePolicy,
        },
        ChangePolicyUpdateParameters {
            parameters: PolicyParameters,
        },
    }

    pub(super) fn proposal_kind(
        generator: &mut schemars::gen::SchemaGenerator,
    ) -> schemars::schema::Schema {
        generator.subschema_for::<ProposalKind>()
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikConfig", deny_unknown_fields)]
    struct Config {
        name: String,
        purpose: String,
        metadata: Base64Bytes,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikActionCall", deny_unknown_fields)]
    struct ActionCall {
        method_name: String,
        args: Base64Bytes,
        deposit: NearToken,
        gas: NearGas,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikBounty", deny_unknown_fields)]
    struct Bounty {
        description: String,
        token: String,
        amount: SU128,
        times: u32,
        max_deadline: SU64,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikFactoryInfo", deny_unknown_fields)]
    struct FactoryInfo {
        factory_id: AccountId,
        auto_update: bool,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikPolicyParameters", deny_unknown_fields)]
    struct PolicyParameters {
        proposal_bond: Option<NearToken>,
        proposal_period: Option<SU64>,
        bounty_bond: Option<NearToken>,
        bounty_forgiveness_period: Option<SU64>,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikProposalStatus")]
    pub(super) enum ProposalStatus {
        InProgress,
        Approved,
        Rejected,
        Removed,
        Expired,
        Moved,
        Failed,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikVote")]
    pub(super) enum Vote {
        Approve,
        Reject,
        Remove,
    }

    #[derive(JsonSchema)]
    #[schemars(rename = "SputnikProposalLog")]
    pub(super) struct ProposalLog {
        block_height: SU64,
    }
}

#[cfg(test)]
mod tests {
    use schemars::{schema_for, JsonSchema};
    use serde_json::{json, Value};

    use super::{AddProposal, Policy, ProposalOutput};

    fn validator<T: JsonSchema>() -> jsonschema::Validator {
        jsonschema::draft7::new(&serde_json::to_value(schema_for!(T)).unwrap()).unwrap()
    }

    #[test]
    fn canonical_proposal_preserves_absent_history_and_numeric_votes() {
        // Finalized get_proposal(0) from templar.sputnik-dao.near, version 2.3.1.
        let wire = json!({
            "id": 0,
            "proposer": "megha19.near",
            "description": "Update Parameters",
            "kind": {"ChangePolicyUpdateParameters": {"parameters": {
                "proposal_bond": "0",
                "proposal_period": "604800000000000",
                "bounty_bond": "100000000000000000000000",
                "bounty_forgiveness_period": "604800000000000"
            }}},
            "status": "Approved",
            "vote_counts": {"council": [1, 0, 0]},
            "votes": {"megha19.near": "Approve"},
            "submission_time": "1731091001220274516"
        });
        let proposal: ProposalOutput = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(proposal.0.id, 0);
        assert!(proposal.0.proposal.last_actions_log.is_none());
        assert_eq!(serde_json::to_value(&proposal).unwrap(), wire);
        assert!(validator::<ProposalOutput>().is_valid(&wire));
    }

    #[test]
    fn proposal_wire_preserves_available_history_and_exact_vote_counts() {
        let schema = validator::<ProposalOutput>();
        for history in [json!([]), json!([{"block_height": "456"}])] {
            for counts in [
                json!([u64::MAX, 0, 0]),
                json!([u128::MAX.to_string(), "0", "0"]),
            ] {
                let wire = json!({
                    "id": 7, "proposer": "member.near", "description": "test",
                    "kind": "Vote", "status": "InProgress",
                    "vote_counts": {"council": counts},
                    "votes": {"member.near": "Approve"}, "submission_time": "123",
                    "last_actions_log": history
                });
                let encoded = serde_json::to_string(&wire).unwrap();
                let proposal: ProposalOutput = serde_json::from_str(&encoded).unwrap();
                assert!(proposal.0.proposal.last_actions_log.is_some());
                assert_eq!(serde_json::to_value(&proposal).unwrap(), wire);
                assert!(schema.is_valid(&wire), "{wire}");
            }
        }
        // deserialize_any cannot represent larger JSON integers exactly.
        for count in ["18446744073709551616", "1.5", "-1"] {
            assert!(serde_json::from_str::<super::VoteCount>(count).is_err());
        }
    }

    #[test]
    fn native_policy_wire_forms_match_schema_without_a_wrapper() {
        let mut native = serde_json::to_value(sputnikdao2::default_policy(vec!["member.near"
            .parse()
            .unwrap()]))
        .unwrap();
        let schema = validator::<Policy>();
        for threshold in [json!("10"), json!([1, 2])] {
            native["default_vote_policy"]["threshold"] = threshold;
            for role_kind in [
                json!("Everyone"),
                json!({"Group": ["member.near"]}),
                json!({"Member": "1"}),
            ] {
                native["roles"][0]["kind"] = role_kind;
                let policy: Policy = serde_json::from_value(native.clone()).unwrap();
                let wire = serde_json::to_value(policy).unwrap();
                assert!(schema.is_valid(&wire), "{wire}");
                assert!(wire.get("proposal_bond").is_some());
            }
        }
        native["default_vote_policy"]["threshold"] = json!([1, 0]);
        assert!(!schema.is_valid(&native));
        assert!(serde_json::from_value::<Policy>(native).is_err());
    }

    #[test]
    fn every_native_proposal_kind_matches_input_and_flattened_output_schemas() {
        let policy = serde_json::to_value(sputnikdao2::default_policy(vec![])).unwrap();
        let role = policy["roles"][0].clone();
        let vote_policy = policy["default_vote_policy"].clone();
        let hash = "11111111111111111111111111111111";
        let kinds: Vec<Value> = vec![
            json!({"ChangeConfig": {"config": {
                "name": "DAO", "purpose": "test", "metadata": "AAEC"
            }}}),
            json!({"ChangePolicy": {"policy": policy}}),
            json!({"ChangePolicy": {"policy": ["member.near"]}}),
            json!({"AddMemberToRole": {"member_id": "member.near", "role": "council"}}),
            json!({"RemoveMemberFromRole": {"member_id": "member.near", "role": "council"}}),
            json!({"FunctionCall": {"receiver_id": "target.near", "actions": [{
                "method_name": "call", "args": "AAEC/w==",
                "deposit": "123", "gas": "30000000000000"
            }]}}),
            json!({"UpgradeSelf": {"hash": hash}}),
            json!({"UpgradeRemote": {
                "receiver_id": "target.near", "method_name": "upgrade", "hash": hash
            }}),
            json!({"Transfer": {
                "token_id": "", "receiver_id": "target.near", "amount": "42", "msg": null
            }}),
            json!({"SetStakingContract": {"staking_id": "staking.near"}}),
            json!({"AddBounty": {"bounty": {
                "description": "bounty", "token": "", "amount": "42",
                "times": 2, "max_deadline": "1000"
            }}}),
            json!({"BountyDone": {"bounty_id": 4, "receiver_id": "target.near"}}),
            json!("Vote"),
            json!({"FactoryInfoUpdate": {"factory_info": {
                "factory_id": "factory.near", "auto_update": false
            }}}),
            json!({"ChangePolicyAddOrUpdateRole": {"role": role}}),
            json!({"ChangePolicyRemoveRole": {"role": "council"}}),
            json!({"ChangePolicyUpdateDefaultVotePolicy": {"vote_policy": vote_policy}}),
            json!({"ChangePolicyUpdateParameters": {"parameters": {
                "proposal_bond": "42", "proposal_period": "1000",
                "bounty_bond": null, "bounty_forgiveness_period": null
            }}}),
        ];
        let input_schema = validator::<AddProposal>();
        let output_schema = validator::<ProposalOutput>();
        for kind in kinds {
            let input = json!({
                "dao_id": "dao.near", "proposal": {"description": "test", "kind": kind}
            });
            let request: AddProposal = serde_json::from_value(input.clone()).unwrap();
            assert!(input_schema.is_valid(&serde_json::to_value(request).unwrap()));
            assert!(input_schema.is_valid(&input), "{input}");

            let output = json!({
                "id": 7, "proposer": "member.near", "description": "test",
                "kind": kind, "status": "InProgress",
                "vote_counts": {"council": ["1", "0", "0"]},
                "votes": {"member.near": "Approve"}, "submission_time": "123",
                "last_actions_log": [{"block_height": "456"}]
            });
            let proposal: ProposalOutput = serde_json::from_value(output.clone()).unwrap();
            assert!(output_schema.is_valid(&serde_json::to_value(proposal).unwrap()));
            assert!(output_schema.is_valid(&output), "{output}");
            assert!(output.get("proposal").is_none());
        }
    }
}
