use super::ContractController;
use crate::{
    controller::storage_management::StorageManagementController, define, get_contract,
    UnifiedMarketController,
};
use near_sdk::{
    json_types::U128,
    serde_json::{self, json},
    AccountId, NearToken,
};
use near_workspaces::{
    network::Sandbox, result::ExecutionSuccess, types::SecretKey, Account, Contract, Worker,
};
use std::ops::Deref;
use templar_common::vault::*;
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
        #[view] pub fn get_max_deposit() -> U128;
        #[view] pub fn get_total_supply() -> U128;
        #[view] pub fn get_idle_balance() -> U128;
        #[view] pub fn get_op_state() -> OpState;
        #[view] pub fn list_supply_queue(offset: Option<u32>, count: Option<u32>) -> Vec<AccountId>;
        #[view] pub fn list_withdraw_queue(offset: Option<u32>, count: Option<u32>) -> Vec<AccountId>;
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
        pub fn allocate["start_allocation"](amount: U128);

        // User withdrawal path; expects escrowed shares already held by the contract.
        #[call(exec, tgas(300))]
        pub fn withdraw["start_withdraw"](amount: U128, receiver: AccountId, owner: AccountId, escrow_shares: U128);

        // User redemption path; expects escrowed shares already held by the contract.
        #[call(exec, tgas(300))]
        pub fn redeem["start_redeem"](shares: U128, receiver: AccountId, owner: AccountId, escrow_shares: U128);

        #[call(exec, tgas(50))]
        pub fn skim["skim"](token: AccountId);

        // TODO: caps?

        /* -------- Promise callbacks (must be #[private] on-chain) -------- */
        // After attempting to supply into a market during allocation.
        #[call(exec, tgas(50))]
        pub fn after_supply_1_check(op_id: u64, index: u32, amount: U128);

        // After creating a withdrawal request on a market during withdrawal orchestration.
        #[call(exec, tgas(50))]
        pub fn after_create_withdraw_req(op_id: u64, index: u32, amount: U128);

        // After payout to the user completes.
        #[call(exec, tgas(50))]
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
        }
    }

    pub fn new(
        vault: VaultController,
        configuration: VaultConfiguration,
        market: UnifiedMarketController,
    ) -> Self {
        Self {
            vault,
            configuration,
            market,
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
        // FIXME: we should set the queue for this too!
    }

    pub async fn supply(&self, supply_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!(
            "{} transferring {amount} tokens for supply...",
            supply_user.id()
        );
        self.market
            .borrow_asset
            .transfer_call(
                supply_user,
                self.vault.contract().id(),
                amount,
                serde_json::to_string(&DepositMsg::Supply).unwrap(),
            )
            .await
    }
}
