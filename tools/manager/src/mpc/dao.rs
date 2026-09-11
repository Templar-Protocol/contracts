//! The Sputnik DAO side of an MPC signing proposal: the `add_proposal` call
//! that requests the signature and the read model of the stored proposal.

use anyhow::Context as _;
use near_account_id::AccountId;
use serde::{Deserialize, Serialize};
use sputnikdao2::{ProposalInput, ProposalStatus};
use templar_gateway_types::{Base64Bytes, NearToken};

use super::signer_contract::{SignArgs, SignRequest};

pub const ADD_PROPOSAL_METHOD: &str = "add_proposal";
pub const GET_PROPOSAL_METHOD: &str = "get_proposal";
pub const GET_POLICY_METHOD: &str = "get_policy";
pub const SIGN_METHOD: &str = "sign";

#[derive(Serialize)]
pub struct AddProposalArgs {
    pub proposal: ProposalInput,
}

#[derive(Serialize)]
pub struct GetProposalArgs {
    pub id: u64,
}

#[derive(Serialize)]
pub struct GetPolicyArgs {}

/// The one policy field `add_proposal` enforces (the deposit must equal it).
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyBond {
    pub proposal_bond: NearToken,
}

/// Read-side mirror of Sputnik's `ProposalOutput`. `sputnikdao2::ActionCall`
/// keeps its fields private, so the kind is re-declared here to reach `args`.
#[derive(Debug, Clone, Deserialize)]
pub struct ProposalView {
    pub id: u64,
    pub proposer: AccountId,
    pub description: String,
    pub status: ProposalStatus,
    kind: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct FunctionCallKind {
    receiver_id: AccountId,
    actions: Vec<ActionCallView>,
}

#[derive(Debug, Clone, Deserialize)]
struct ActionCallView {
    method_name: String,
    args: Base64Bytes,
}

/// The `sign` call a proposal executes: which MPC contract, with which request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedSignature {
    pub mpc_contract_id: AccountId,
    pub request: SignRequest,
}

impl ProposalView {
    pub fn proposed_signature(&self) -> anyhow::Result<ProposedSignature> {
        let kind = self
            .kind
            .get("FunctionCall")
            .with_context(|| format!("proposal {} is not a FunctionCall proposal", self.id))?;
        let kind: FunctionCallKind =
            serde_json::from_value(kind.clone()).context("decode the FunctionCall kind")?;
        let [action] = kind.actions.as_slice() else {
            anyhow::bail!(
                "proposal {} carries {} actions; an MPC signing proposal carries exactly one `{SIGN_METHOD}` call",
                self.id,
                kind.actions.len()
            );
        };
        anyhow::ensure!(
            action.method_name == SIGN_METHOD,
            "proposal {} calls `{}`, not `{SIGN_METHOD}`",
            self.id,
            action.method_name
        );
        let args: SignArgs =
            serde_json::from_slice(&action.args.0).context("decode the `sign` arguments")?;
        Ok(ProposedSignature {
            mpc_contract_id: kind.receiver_id,
            request: args.request,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpc::signer_contract::Payload;

    fn proposal(kind: &serde_json::Value) -> ProposalView {
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "proposer": "peer.near",
            "description": "sign it",
            "kind": kind,
            "status": "Approved",
            "vote_counts": {},
            "votes": {},
            "submission_time": "0",
        }))
        .expect("parses")
    }

    /// Proposal 1 on `templar-testing.sputnik-dao.near`, as `get_proposal` returns it.
    #[test]
    fn reads_the_sign_request_out_of_a_real_proposal() {
        let args = serde_json::json!({
            "request": {
                "payload_v2": { "Eddsa": "8808035030bdd4cdd5b71353b41c94b70a6c9cf38fb05e0d34170be230d87a8c" },
                "path": "templar-testing.sputnik-dao.near-mpc-test.peer2f00l.near",
                "domain_id": 1,
            }
        });
        let proposal = proposal(&serde_json::json!({
            "FunctionCall": {
                "receiver_id": "v1.signer",
                "actions": [{
                    "method_name": "sign",
                    "args": Base64Bytes(serde_json::to_vec(&args).expect("json")),
                    "deposit": "1",
                    "gas": "15000000000000",
                }],
            }
        }));

        let proposed = proposal.proposed_signature().expect("a sign proposal");
        assert_eq!(proposed.mpc_contract_id.as_str(), "v1.signer");
        assert_eq!(proposed.request.domain_id, 1);
        assert!(matches!(proposed.request.payload, Payload::Eddsa(_)));
        assert_eq!(proposal.status, ProposalStatus::Approved);
    }

    #[test]
    fn a_non_function_call_proposal_is_refused() {
        let proposal = proposal(&serde_json::json!({
            "Transfer": { "token_id": "", "receiver_id": "x.near", "amount": "1" }
        }));
        assert!(proposal.proposed_signature().is_err());
    }

    #[test]
    fn a_function_call_to_something_else_is_refused() {
        let proposal = proposal(&serde_json::json!({
            "FunctionCall": {
                "receiver_id": "market.near",
                "actions": [{ "method_name": "borrow", "args": "e30=", "deposit": "0", "gas": "1" }],
            }
        }));
        let error = proposal.proposed_signature().expect_err("not a sign call");
        assert!(error.to_string().contains("`borrow`"), "{error}");
    }
}
