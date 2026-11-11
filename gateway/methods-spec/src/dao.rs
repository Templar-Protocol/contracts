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
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[schemars(rename = "SputnikProposalOutput")]
pub struct ProposalRecord {
    pub id: u64,
    #[serde(flatten)]
    pub proposal: Proposal,
}

impl<'de> Deserialize<'de> for ProposalRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{DeserializeSeed, Error, IntoDeserializer, MapAccess, Visitor};

        // Stream the flattened body: serde's flatten buffer cannot preserve
        // RawValue counts for exact integer and signed-zero checks.
        struct ProposalFields<M> {
            map: M,
            id: Option<u64>,
        }

        impl<'de, M: MapAccess<'de>> MapAccess<'de> for ProposalFields<M> {
            type Error = M::Error;

            fn next_key_seed<K: DeserializeSeed<'de>>(
                &mut self,
                seed: K,
            ) -> Result<Option<K::Value>, Self::Error> {
                while let Some(key) = self.map.next_key::<String>()? {
                    if key == "id" {
                        if self.id.is_some() {
                            return Err(M::Error::duplicate_field("id"));
                        }
                        self.id = Some(self.map.next_value()?);
                    } else {
                        return seed.deserialize(key.into_deserializer()).map(Some);
                    }
                }
                Ok(None)
            }

            fn next_value_seed<V: DeserializeSeed<'de>>(
                &mut self,
                seed: V,
            ) -> Result<V::Value, Self::Error> {
                self.map.next_value_seed(seed)
            }
        }

        struct RecordVisitor;

        impl<'de> Visitor<'de> for RecordVisitor {
            type Value = ProposalRecord;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a flattened Sputnik proposal with its ID")
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                let mut fields = ProposalFields { map, id: None };
                let proposal = Proposal::deserialize(
                    serde::de::value::MapAccessDeserializer::new(&mut fields),
                )?;
                let id = fields.id.ok_or_else(|| M::Error::missing_field("id"))?;
                Ok(ProposalRecord { id, proposal })
            }
        }

        deserializer.deserialize_map(RecordVisitor)
    }
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

/// Preserve canonical small numeric counts and modern decimal-string counts.
///
/// Canonical counts above u64 are normalized to decimal strings, preserving their
/// full u128 value without changing shared JSON-number decoding or rounding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "SputnikVoteCount")]
pub enum VoteCount {
    Number(u64),
    Decimal(SU128),
}

impl<'de> Deserialize<'de> for VoteCount {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
        let raw = raw.get().trim_start();
        if raw.starts_with('"') {
            return serde_json::from_str::<SU128>(raw)
                .map(Self::Decimal)
                .map_err(serde::de::Error::custom);
        }
        let count = serde_json::from_str::<u128>(raw).map_err(serde::de::Error::custom)?;
        Ok(match u64::try_from(count) {
            Ok(count) => Self::Number(count),
            Err(_) => Self::Decimal(count.into()),
        })
    }
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

    use super::{AddProposal, Policy, ProposalOutput, VoteCount};

    fn validator<T: JsonSchema>() -> jsonschema::Validator {
        jsonschema::draft7::new(&serde_json::to_value(schema_for!(T)).unwrap()).unwrap()
    }

    fn vote_proposal(counts: &Value, history: Option<Value>) -> Value {
        let mut wire = json!({
            "id": 7, "proposer": "member.near", "description": "test",
            "kind": "Vote", "status": "InProgress",
            "vote_counts": {"council": counts},
            "votes": {"member.near": "Approve"}, "submission_time": "123"
        });
        if let Some(history) = history {
            wire["last_actions_log"] = history;
        }
        wire
    }

    fn raw_vote_proposal(count: &str) -> String {
        format!(
            r#"{{"id":7,"proposer":"member.near","description":"test",
            "kind":"Vote","status":"InProgress",
            "vote_counts":{{"council":[{count},0,0]}},
            "votes":{{"member.near":"Approve"}},"submission_time":"123"}}"#
        )
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
    fn canonical_u128_counts_normalize_without_rounding() {
        let schema = validator::<ProposalOutput>();
        for count in [
            0_u128,
            9_007_199_254_740_993,
            u128::from(u64::MAX),
            u128::from(u64::MAX) + 1,
            u128::MAX,
        ] {
            let proposal: ProposalOutput =
                serde_json::from_str(&raw_vote_proposal(&count.to_string())).unwrap();
            let expected = match u64::try_from(count) {
                Ok(count) => VoteCount::Number(count),
                Err(_) => VoteCount::Decimal(count.into()),
            };
            assert_eq!(proposal.0.proposal.vote_counts["council"][0], expected);
            let wire = serde_json::to_value(&proposal).unwrap();
            let serialized_count = &wire["vote_counts"]["council"][0];
            if let Ok(count) = u64::try_from(count) {
                assert_eq!(serialized_count.as_u64(), Some(count));
            } else {
                assert_eq!(serialized_count.as_str(), Some(count.to_string().as_str()));
            }
            assert!(schema.is_valid(&wire));
            let round_trip: ProposalOutput = serde_json::from_value(wire).unwrap();
            assert_eq!(round_trip.0.proposal.vote_counts["council"][0], expected);
        }
    }

    #[test]
    fn decimal_counts_preserve_available_and_absent_history() {
        for history in [
            None,
            Some(json!([])),
            Some(json!([{"block_height": "456"}])),
        ] {
            let wire = vote_proposal(&json!([u128::MAX.to_string(), "0", "0"]), history);
            let proposal: ProposalOutput = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(
                proposal.0.proposal.vote_counts["council"][0],
                VoteCount::Decimal(u128::MAX.into()),
            );
            assert_eq!(serde_json::to_value(proposal).unwrap(), wire);
        }
    }

    #[test]
    fn flattened_proposal_rejects_invalid_vote_counts() {
        for token in [
            "-1",
            "-0",
            "1.5",
            "1.0",
            "1e0",
            "1e1000",
            "340282366920938463463374607431768211456",
            "\"340282366920938463463374607431768211456\"",
            "\"-1\"",
            "\"1.5\"",
            "\"1e0\"",
            "\"\"",
            "true",
            "null",
            "[]",
            "{}",
        ] {
            let encoded = raw_vote_proposal(token);
            assert!(
                serde_json::from_str::<ProposalOutput>(&encoded).is_err(),
                "{token}",
            );
            if let Ok(wire) = serde_json::from_str::<Value>(&encoded) {
                assert!(
                    serde_json::from_value::<ProposalOutput>(wire).is_err(),
                    "{token}",
                );
            }
        }
        for token in ["-0", "01", "+1", "NaN", "1.", "1e"] {
            assert!(serde_json::from_str::<VoteCount>(token).is_err(), "{token}");
        }
    }

    #[test]
    fn flattened_proposal_rejects_duplicate_known_fields() {
        let wire = vote_proposal(&json!([u64::MAX, 0, 0]), Some(json!([])));
        let encoded = serde_json::to_string(&wire).unwrap();
        let prefix = encoded.strip_suffix('}').unwrap();
        for (field, value) in wire.as_object().unwrap() {
            let duplicate = format!(
                "{prefix},{}:{value}}}",
                serde_json::to_string(field).unwrap(),
            );
            assert!(
                serde_json::from_str::<ProposalOutput>(&duplicate).is_err(),
                "{field}",
            );
        }
    }

    #[test]
    fn flattened_proposal_retains_native_enum_duplicate_checks() {
        let encoded = r#"{
            "id": 7, "proposer": "member.near", "description": "test",
            "kind": {"Transfer": {
                "token_id": "", "receiver_id": "member.near",
                "amount": "1", "amount": "2", "msg": null
            }},
            "status": "InProgress", "vote_counts": {"council": [0, 0, 0]},
            "votes": {}, "submission_time": "123"
        }"#;
        assert!(serde_json::from_str::<ProposalOutput>(encoded).is_err());
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
        }
    }
}
