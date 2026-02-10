use std::{env, ops::Deref};

use near_api::types::transaction::result::ExecutionSuccess;
use near_sdk::{
    json_types::{U128, U64},
    serde_json::{self, json},
    AccountId, NearToken,
};
use tokio::sync::OnceCell;

use templar_common::vault::{AllocationDelta, DepositMsg, VaultConfiguration};

use super::ContractController;
use crate::{
    define, get_contract, print_execution, StorageManagementController, TestAccount,
    UnifiedMarketController,
};

#[derive(Clone)]
pub struct VaultController {
    account: TestAccount,
}

impl ContractController for VaultController {
    fn account(&self) -> &TestAccount {
        &self.account
    }
}

impl StorageManagementController for VaultController {}

impl VaultController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();
        WASM.get_or_init(|| get_contract("templar_vault_contract", "contract/vault"))
            .await
    }

    pub async fn deploy(account: TestAccount, configuration: &VaultConfiguration) -> Self {
        let init_call = near_api::Contract::deploy(account.id.clone())
            .use_code(Self::wasm().await.to_vec())
            .with_init_call("new", json!({ "configuration": configuration }))
            .unwrap()
            .with_signer(account.signer())
            .send_to(&account.network)
            .await
            .unwrap()
            .assert_success();

        eprintln!("Init call logs");
        eprintln!("--------------");
        for log in init_call.logs() {
            eprintln!("\t{log}");
        }
        eprintln!("--------------");

        Self { account }
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
        pub fn reallocate(delta: AllocationDelta);

        // Allocator-only: executes an existing market-side supply withdrawal
        // request and credits any returned funds to the vault's idle balance.
        #[call(exec, tgas(300))]
        pub fn execute_rebalance_withdrawal(market: AccountId, batch_limit: Option<u32>);

        #[call(exec, tgas(30), deposit(NearToken::from_yoctonear(2560000000000000000000)))]
        pub fn withdraw(amount: U128, receiver: AccountId);

        #[call(exec, tgas(300))]
        pub fn execute_withdrawal(route: Vec<AccountId>);

        #[call(exec, tgas(300))]
        pub fn execute_market_withdrawal(op_id: U64, market_index: u32, batch_limit: Option<u32>);

        #[call(exec, tgas(300))]
        pub fn unbrick["unbrick"]();

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

macro_rules! debug_method {
    ($(
        fn $n:ident ($($a:ident : $t:ty),*);
    )*) => {
        $(
            debug_method! {
                fn $n ($($a : $t),*)
            }
        )*
    };
    (fn $n:ident ($($a:ident : $t:ty),*)) => {
        pub async fn $n(
            &self,
            $( $a : $t ),*
        ) -> ExecutionSuccess {
            let e = self.vault.$n($($a),*).await;
            if self.debug {
                print_execution(&e);
            }
            e
        }
    };
}

impl UnifiedVaultController {
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

    pub async fn init_account(&self, account: &TestAccount) {
        self.storage_deposits(account).await;
        self.market.init_account(account).await;
    }

    pub async fn storage_deposits(&self, account: &TestAccount) {
        eprintln!("Performing storage deposits for {}...", account.id());
        let bounds = self.vault.storage_balance_bounds().await;

        self.vault.storage_deposit(account, bounds.min).await;
        self.market.storage_deposits(account).await;
    }

    pub async fn supply(&self, supply_user: &TestAccount, amount: u128) -> ExecutionSuccess {
        eprintln!(
            "{} transferring {amount} tokens for supply...",
            supply_user.id()
        );
        let e = self
            .market
            .borrow_asset
            .transfer_call(
                supply_user,
                &self.vault.account.id,
                amount,
                serde_json::to_string(&DepositMsg::Supply).unwrap(),
            )
            .await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn setup_caps(&self, owner: &TestAccount, markets: &[AccountId], amount: u128) {
        for mkt in markets {
            self.submit_cap(owner, mkt.clone(), amount.into()).await;
            self.accept_cap(owner, mkt.clone()).await;
        }

        self.set_supply_queue(owner, markets).await;
    }

    pub async fn allocate(
        &self,
        allocator: &TestAccount,
        delta: AllocationDelta,
    ) -> ExecutionSuccess {
        let e = self.vault.reallocate(allocator, delta).await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    pub async fn withdraw(
        &self,
        withdrawer: &TestAccount,
        amount: U128,
        receiver: Option<AccountId>,
    ) -> ExecutionSuccess {
        let e = self
            .vault
            .withdraw(
                withdrawer,
                amount,
                receiver.unwrap_or(withdrawer.id.clone()),
            )
            .await;
        if self.debug {
            print_execution(&e);
        }
        e
    }

    debug_method! {
        fn execute_rebalance_withdrawal(allocator: &TestAccount, market: AccountId, batch_limit: Option<u32>);
        fn execute_withdrawal(allocator: &TestAccount, route: Vec<AccountId>);
        fn execute_market_withdrawal(allocator: &TestAccount, op_id: U64, market_index: u32, batch_limit: Option<u32>);
        fn submit_cap(submitter: &TestAccount, market: AccountId, amount: U128);
        fn accept_cap(acceptor: &TestAccount, market: AccountId);
        fn set_supply_queue(allocator: &TestAccount, markets: &[AccountId]);
        fn set_withdraw_queue(allocator: &TestAccount, markets: &[AccountId]);
    }
}

fn is_debug() -> bool {
    env::var("RUST_LOG").is_ok_and(|s| s.contains("debug")) || env::var("DEBUG").is_ok()
}
