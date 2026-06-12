use near_sdk::json_types::{Base64VecU8, U128};
use near_sdk::serde_json::json;
use near_sdk::Gas;
use near_workspaces::{Account, Contract};
use templar_common::Nanoseconds;
use templar_proxy_oracle_near_governance_common::{
    Operation, OperationKind, Proposal, Role, TtlConfig,
};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

pub struct GovernanceContractController {
    pub contract: Contract,
}

impl ContractController for GovernanceContractController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl GovernanceContractController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| {
            get_contract(
                "templar_proxy_oracle_near_governance_contract",
                "contract/proxy-oracle/near/governance-contract",
            )
        })
        .await
    }

    pub async fn deploy(
        account: Account,
        proxy_oracle_id: near_sdk::AccountId,
        admin_id: near_sdk::AccountId,
        ttls: TtlConfig,
    ) -> Self {
        let contract = account
            .deploy(Self::wasm().await)
            .await
            .expect("governance contract deploy RPC failed")
            .into_result()
            .expect("governance contract deploy transaction failed");
        contract
            .call("new")
            .args_json(
                json!({ "proxy_oracle_id": proxy_oracle_id, "admin_id": admin_id, "ttls": ttls }),
            )
            .transact()
            .await
            .expect("governance contract init RPC failed")
            .into_result()
            .expect("governance contract init transaction failed");

        Self { contract }
    }

    pub async fn admin_upgrade(
        &self,
        executor: &Account,
        id: u32,
        code: Vec<u8>,
        migrate_args: Vec<u8>,
        requested_ttl: Nanoseconds,
    ) {
        self.create_proposal(
            executor,
            id,
            Operation::AdminUpgrade {
                code: Base64VecU8(code),
                migrate_args: Base64VecU8(migrate_args),
            },
            requested_ttl,
        )
        .await;
        self.execute_proposal(executor, id).await;
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn admin_function_call(
        &self,
        executor: &Account,
        id: u32,
        method_name: String,
        args: Vec<u8>,
        attached_deposit_yocto: u128,
        gas: Gas,
        requested_ttl: Nanoseconds,
    ) {
        self.create_proposal(
            executor,
            id,
            Operation::AdminFunctionCall {
                method_name,
                args: Base64VecU8(args),
                attached_deposit: U128(attached_deposit_yocto),
                gas,
            },
            requested_ttl,
        )
        .await;
        self.execute_proposal(executor, id).await;
    }

    define! {
        #[view] pub fn next_proposal_id() -> u32;
        #[view] pub fn proposal_count() -> u32;
        #[view] pub fn list_proposals(offset: Option<u32>, count: Option<u32>) -> Vec<u32>;
        #[view] pub fn get_proposal(id: u32) -> Option<Proposal<Operation>>;
        #[view] pub fn get_operation_ttl(kind: OperationKind) -> Nanoseconds;
        #[view] pub fn get_effective_proposal_ttl(operation: Operation, requested_ttl: Nanoseconds) -> Nanoseconds;
        #[view] pub fn has_role(account_id: near_sdk::AccountId, role: Role) -> bool;
        #[view] pub fn list_role(role: Role, offset: Option<u32>, count: Option<u32>) -> Vec<near_sdk::AccountId>;
        #[view] pub fn get_roles(account_id: near_sdk::AccountId) -> Vec<Role>;

        #[call(yocto(1))]
        pub fn create_proposal(id: u32, operation: Operation, requested_ttl: Nanoseconds) -> Proposal<Operation>;
        #[call(exec, yocto(1))]
        pub fn cancel_proposal(id: u32);
        #[call(exec, yocto(1))]
        pub fn execute_proposal(id: u32);
    }
}
