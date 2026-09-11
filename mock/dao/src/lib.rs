//! Stand-in for a Sputnik DAO v2, reduced to what an MPC signing proposal
//! touches: `add_proposal` with an exact bond, `get_proposal`/`get_policy`
//! reads, and `act_proposal` approving and executing a FunctionCall kind.
//! JSON shapes match Sputnik's so the CLI reads it unchanged.

// `#[near]` method parameters are taken by value; the generated wrappers own them.
#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    env, is_promise_success,
    json_types::{Base64VecU8, U64},
    near,
    store::Vector,
    AccountId, Gas, NearToken, PanicOnDefault, Promise,
};

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub struct ActionCall {
    pub method_name: String,
    pub args: Base64VecU8,
    pub deposit: NearToken,
    pub gas: Gas,
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub enum ProposalKind {
    FunctionCall {
        receiver_id: AccountId,
        actions: Vec<ActionCall>,
    },
}

#[near(serializers = [json, borsh])]
#[derive(Clone, PartialEq, Eq)]
pub enum ProposalStatus {
    InProgress,
    Approved,
    Failed,
}

#[near(serializers = [json])]
pub struct ProposalInput {
    pub description: String,
    pub kind: ProposalKind,
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub struct Proposal {
    pub proposer: AccountId,
    pub description: String,
    pub kind: ProposalKind,
    pub status: ProposalStatus,
    pub submission_time: U64,
}

#[near(serializers = [json])]
pub struct ProposalOutput {
    pub id: u64,
    #[serde(flatten)]
    pub proposal: Proposal,
}

#[near(serializers = [json])]
pub struct Policy {
    pub proposal_bond: NearToken,
    pub proposal_period: U64,
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    proposals: Vector<Proposal>,
    proposal_bond: NearToken,
}

#[near]
impl Contract {
    #[init]
    pub fn new(proposal_bond: NearToken) -> Self {
        Self {
            proposals: Vector::new(b"p"),
            proposal_bond,
        }
    }

    #[payable]
    pub fn add_proposal(&mut self, proposal: ProposalInput) -> u64 {
        assert_eq!(env::attached_deposit(), self.proposal_bond, "ERR_MIN_BOND");
        let id = self.proposals.len();
        self.proposals.push(Proposal {
            proposer: env::predecessor_account_id(),
            description: proposal.description,
            kind: proposal.kind,
            status: ProposalStatus::InProgress,
            submission_time: U64(env::block_timestamp()),
        });
        u64::from(id)
    }

    pub fn get_proposal(&self, id: u64) -> ProposalOutput {
        ProposalOutput {
            id,
            proposal: self.proposal(id).clone(),
        }
    }

    pub fn get_last_proposal_id(&self) -> u64 {
        u64::from(self.proposals.len())
    }

    pub fn get_policy(&self) -> Policy {
        Policy {
            proposal_bond: self.proposal_bond,
            proposal_period: U64(7 * 24 * 60 * 60 * 1_000_000_000),
        }
    }

    /// Every caller is council with a threshold of one: `VoteApprove` executes.
    pub fn act_proposal(&mut self, id: u64, action: String) -> Promise {
        assert_eq!(action, "VoteApprove", "only VoteApprove is mocked");
        let proposal = self.proposal(id).clone();
        assert!(
            proposal.status == ProposalStatus::InProgress,
            "proposal is not in progress"
        );
        let ProposalKind::FunctionCall {
            receiver_id,
            actions,
        } = proposal.kind;
        let mut promise = Promise::new(receiver_id);
        for action in actions {
            promise = promise.function_call(
                action.method_name,
                Vec::<u8>::from(action.args),
                action.deposit,
                action.gas,
            );
        }
        promise.then(
            Self::ext(env::current_account_id())
                .with_static_gas(Gas::from_tgas(10))
                .on_proposal_callback(id),
        )
    }

    #[private]
    pub fn on_proposal_callback(&mut self, id: u64) {
        let status = if is_promise_success() {
            ProposalStatus::Approved
        } else {
            ProposalStatus::Failed
        };
        let index = u32::try_from(id).unwrap_or_else(|_| env::panic_str("proposal id"));
        let proposal = self
            .proposals
            .get_mut(index)
            .unwrap_or_else(|| env::panic_str("ERR_NO_PROPOSAL"));
        proposal.status = status;
    }

    fn proposal(&self, id: u64) -> &Proposal {
        let index = u32::try_from(id).unwrap_or_else(|_| env::panic_str("proposal id"));
        self.proposals
            .get(index)
            .unwrap_or_else(|| env::panic_str("ERR_NO_PROPOSAL"))
    }
}

#[cfg(target_arch = "wasm32")]
mod custom_getrandom {
    #![allow(clippy::no_mangle_with_rust_abi)]

    use getrandom::{register_custom_getrandom, Error};
    use near_sdk::env;

    register_custom_getrandom!(custom_getrandom);

    #[allow(clippy::unnecessary_wraps)]
    pub fn custom_getrandom(buf: &mut [u8]) -> Result<(), Error> {
        buf.copy_from_slice(&env::random_seed_array());
        Ok(())
    }
}
