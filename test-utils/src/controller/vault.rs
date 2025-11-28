use super::ContractController;
use crate::{
    controller::storage_management::StorageManagementController, define, get_contract,
    print_execution, UnifiedMarketController,
};
use near_sdk::{
    json_types::{U128, U64},
    serde_json::{self, json},
    AccountId, NearToken,
};
use near_workspaces::{
    network::Sandbox, result::ExecutionSuccess, types::SecretKey, Account, Contract, Worker,
};
use std::{env, ops::Deref};
use templar_common::vault::{AllocationWeights, DepositMsg, VaultConfiguration};
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct VaultController {
    contract: Contract,
}

impl ContractController for VaultController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl StorageManagementController for VaultController {}

impl VaultController {
    pub async fn deploy(account: Account, configuration: &VaultConfiguration) -> Self {
        let wasm = load_wasm().await;
        let contract = account.deploy(wasm).await.unwrap().unwrap();

        let init_call = contract
            .call("new")
            .args_json(json!({
                "configuration": configuration,
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        eprintln!("Init call logs");
        eprintln!("--------------");
        for log in init_call.logs() {
            eprintln!("\t{log}");
        }
        eprintln!("--------------");

        Self { contract }
    }

    define! {
        /* -------- Views -------- */
        #[view] pub fn get_configuration() -> VaultConfiguration;
        #[view] pub fn get_fee_recipient() -> AccountId;
        #[view] pub fn get_last_total_assets() -> U128;
        #[view] pub fn get_total_assets() -> U128;
        #[view] pub fn get_total_supply() -> U128;
        #[view] pub fn get_max_deposit() -> U128;
        #[view] pub fn get_idle_balance() -> U128;
        #[view] pub fn get_withdrawing_op_id() -> Option<U64>;
        #[view] pub fn get_current_withdraw_request_id() -> Option<U64>;
        #[view] pub fn has_pending_market_withdrawal() -> bool;


        #[view] pub fn get_market_supply(market: &AccountId) -> U128;
        #[view] pub fn get_next_op_id() -> u64;
        #[view] pub fn convert_to_shares(assets: U128) -> U128;
        #[view] pub fn convert_to_assets(shares: U128) -> U128;
        #[view] pub fn preview_mint(shares: U128) -> U128;
        #[view] pub fn preview_deposit(assets: U128) -> U128;
        #[view] pub fn preview_withdraw(assets: U128) -> U128;
        #[view] pub fn preview_redeem(shares: U128) -> U128;

        /* -------- Calls (externals) -------- */
        // Owner/guardian-gated: mints fee shares when performance is positive.
        #[call(exec, tgas(20))]
        pub fn accrue_fee["internal_accrue_fee"]();

        // Allocator/curator/owner-gated: begins allocation across markets.
        #[call(exec, tgas(300))]
        pub fn allocate(weights: AllocationWeights, amount: Option<U128>);

        #[call(exec, tgas(30), deposit(NearToken::from_yoctonear(2560000000000000000000)))]
        pub fn withdraw(amount: U128, receiver: AccountId);

        #[call(exec, tgas(300))]
        pub fn execute_next_withdrawal_request(route: Vec<AccountId>);

        #[call(exec, tgas(300))]
        pub fn execute_next_market_withdrawal(op_id: U64);

        #[call(exec, tgas(300), deposit(NearToken::from_yoctonear(2560000000000000000000)))]
        pub fn redeem(shares: U128, receiver: AccountId);

        #[call(exec, tgas(50))]
        pub fn skim["skim"](token: AccountId);

        #[call(exec, tgas(5))]
        pub fn submit_cap(market: AccountId, new_cap: U128);

        #[call(exec, tgas(5))]
        pub fn accept_cap(market: AccountId);

        #[call(exec, tgas(5))]
        pub fn revoke_pending_cap(market: AccountId);

        #[call(exec, tgas(50))]
        pub fn submit_market_removal(market: AccountId);

        #[call(exec, tgas(50))]
        pub fn revoke_pending_market_removal(market: AccountId);

        #[call(exec, tgas(50))]
        pub fn set_curator(account: AccountId);

        #[call(exec, tgas(50))]
        pub fn set_is_allocator(account: AccountId, allowed: bool);

        #[call(exec, tgas(50))]
        pub fn submit_guardian(new_g: AccountId);

        #[call(exec, tgas(50))]
        pub fn accept_guardian();

        #[call(exec, tgas(50))]
        pub fn revoke_pending_guardian();

        #[call(exec, tgas(50))]
        pub fn set_skim_recipient(account: AccountId);

        #[call(exec, tgas(50))]
        pub fn set_fee_recipient(account: AccountId);

        #[call(exec, tgas(50))]
        pub fn set_performance_fee(fee: U128);

        #[call(exec, tgas(50))]
        pub fn submit_timelock(new_timelock_ns: U64);

        #[call(exec, tgas(50))]
        pub fn accept_timelock();

        #[call(exec, tgas(50))]
        pub fn revoke_pending_timelock();

        #[call(exec, tgas(50), deposit(NearToken::from_yoctonear(840000000000000000000)))]
        pub fn set_supply_queue(markets: Vec<AccountId>);

        #[call(exec, tgas(50))]
        pub fn set_withdraw_queue(queue: Vec<AccountId>);


        // After attempting to supply into a market during allocation.
        #[call(exec, tgas(30))]
        pub fn after_supply_1_check(op_id: u64, index: u32, amount: U128);

        // After creating a withdrawal request on a market during withdrawal orchestration.
        #[call(exec, tgas(20))]
        pub fn after_create_withdraw_req(op_id: u64, index: u32, amount: U128);

        // After payout to the user completes.
        #[call(exec, tgas(5))]
        pub fn after_send_to_user(op_id: u64, receiver: AccountId, amount: U128);
    }
}

static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

pub async fn load_wasm() -> &'static [u8] {
    WASM.get_or_init(|| get_contract("templar_vault_contract", "contract/vault"))
        .await
}

#[derive(Clone)]
pub struct UnifiedVaultController {
    pub vault: VaultController,
    pub configuration: VaultConfiguration,
    pub market: UnifiedMarketController,
    pub debug: bool,
}

impl Deref for UnifiedVaultController {
    type Target = VaultController;

    fn deref(&self) -> &Self::Target {
        &self.vault
    }
}

fn contract_with_dummy_sk(worker: &Worker<Sandbox>, account_id: AccountId) -> Contract {
    let dummy_key = SecretKey::from_seed(near_workspaces::types::KeyType::ED25519, "");

    Contract::from_secret_key(account_id, dummy_key.clone(), worker)
}

impl UnifiedVaultController {
    pub async fn attach(worker: &Worker<Sandbox>, market_id: AccountId) -> Self {
        let vault = VaultController {
            contract: contract_with_dummy_sk(worker, market_id.clone()),
        };
        let market = UnifiedMarketController::attach(worker, market_id).await;

        let configuration = vault.get_configuration().await;

        Self {
            vault,
            configuration,
            market,
            debug: is_debug(),
        }
    }

    #[must_use]
    pub fn new(
        vault: VaultController,
        configuration: VaultConfiguration,
        market: UnifiedMarketController,
    ) -> Self {
        Self {
            vault,
            configuration,
            market,
            debug: is_debug(),
        }
    }

    pub async fn init_account(&self, account: &Account) {
        self.storage_deposits(account).await;
        self.market.init_account(account).await;
    }

    pub async fn storage_deposits(&self, account: &Account) {
        eprintln!("Performing storage deposits for {}...", account.id());
        let bounds = self.vault.storage_balance_bounds().await;

        self.vault.storage_deposit(account, bounds.min).await;
        self.market.storage_deposits(account).await;
    }

    pub async fn supply(&self, supply_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!(
            "{} transferring {amount} tokens for supply...",
            supply_user.id()
        );
        let e = self
            .market
            .borrow_asset
            .transfer_call(
                supply_user,
                self.vault.contract().id(),
                amount,
                serde_json::to_string(&DepositMsg::Supply).unwrap(),
            )
            .await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn setup_caps(&self, owner: &Account, markets: &[AccountId], amount: u128) {
        for mkt in markets {
            self.submit_cap(owner, mkt.clone(), amount.into()).await;
            self.accept_cap(owner, mkt.clone()).await;
        }

        self.set_supply_queue(owner, markets).await;
    }

    pub async fn allocate(
        &self,
        allocator: &Account,
        weights: AllocationWeights,
        amount: Option<U128>,
    ) -> ExecutionSuccess {
        let e = self
            .vault
            .allocate(allocator, weights, amount.unwrap_or(1000.into()))
            .await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn withdraw(
        &self,
        withdrawer: &Account,
        amount: U128,
        receiver: Option<AccountId>,
    ) -> ExecutionSuccess {
        let e = self
            .vault
            .withdraw(
                withdrawer,
                amount,
                receiver.unwrap_or(withdrawer.id().clone()),
            )
            .await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn execute_next_withdrawal(
        &self,
        allocator: &Account,
        route: Vec<AccountId>,
    ) -> ExecutionSuccess {
        let e = self
            .vault
            .execute_next_withdrawal_request(allocator, route)
            .await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn execute_next_market_withdrawal(
        &self,
        allocator: &Account,
        op_id: U64,
    ) -> ExecutionSuccess {
        let e = self
            .vault
            .execute_next_market_withdrawal(allocator, op_id)
            .await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn submit_cap(
        &self,
        submitter: &Account,
        market: AccountId,
        amount: U128,
    ) -> ExecutionSuccess {
        let e = self.vault.submit_cap(submitter, market, amount).await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn accept_cap(&self, acceptor: &Account, market: AccountId) -> ExecutionSuccess {
        let e = self.vault.accept_cap(acceptor, market).await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn set_supply_queue(
        &self,
        allocator: &Account,
        markets: &[AccountId],
    ) -> ExecutionSuccess {
        let e = self.vault.set_supply_queue(allocator, markets).await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn set_withdraw_queue(
        &self,
        allocator: &Account,
        markets: &[AccountId],
    ) -> ExecutionSuccess {
        let e = self.vault.set_withdraw_queue(allocator, markets).await;
        if self.debug {
            print_execution(&e);
        }
        e
    }
}

fn is_debug() -> bool {
    env::var("RUST_LOG").is_ok_and(|s| s.contains("debug")) || env::var("DEBUG").is_ok()
}
