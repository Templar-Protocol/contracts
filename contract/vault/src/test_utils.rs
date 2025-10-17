use crate::Contract;
use near_sdk::env;
use near_sdk::NearToken;
pub use near_sdk::{
    test_utils::{accounts, VMContextBuilder},
    test_vm_config, testing_env, AccountId, PromiseResult, RuntimeFeesConfig,
};
use near_sdk_contract_tools::ft::Nep141Controller as _;
use templar_common::primitive_types::U128;
use test_utils::vault_configuration;

pub fn mk(n: u32) -> AccountId {
    format!("acc{n}.testnet").parse().unwrap()
}

pub fn setup_env(
    current: &AccountId,
    predecessor: &AccountId,
    promise_results: Vec<PromiseResult>,
) {
    let mut builder = VMContextBuilder::new();
    builder.current_account_id(current.clone());
    builder.predecessor_account_id(predecessor.clone());
    builder.signer_account_id(predecessor.clone());
    testing_env!(
        builder.build(),
        test_vm_config(),
        RuntimeFeesConfig::test(),
        Default::default(),
        promise_results
    );
}

pub fn new_test_contract(vault_id: &AccountId) -> Contract {
    setup_env(vault_id, vault_id, vec![]);

    // Basic accounts
    let owner = accounts(1);
    let curator = accounts(2);
    let guardian = accounts(3);
    let fee_recipient = accounts(4);
    let skim_recipient = accounts(5);
    let underlying_token_id = mk(6);

    let cfg = vault_configuration(
        owner,
        curator,
        guardian,
        underlying_token_id,
        skim_recipient,
        fee_recipient,
    );

    Contract::new(cfg)
}
/// Set the block timestamp and keep caller/predecessor consistent for tests
pub fn set_block_ts(vault_id: &AccountId, signer: &AccountId, ts: u64) {
    set_ctx(vault_id, signer, Some(ts), None);
}

pub fn set_ctx(vault_id: &AccountId, signer: &AccountId, ts: Option<u64>, deposit: Option<u128>) {
    let mut ctx = VMContextBuilder::new();
    ctx.current_account_id(vault_id.clone());
    ctx.signer_account_id(signer.clone());
    ctx.predecessor_account_id(signer.clone());
    if let Some(ts) = ts {
        ctx.block_timestamp(ts);
    }
    if let Some(amount) = deposit {
        ctx.attached_deposit(NearToken::from_yoctonear(amount));
    }
    testing_env!(ctx.build());
}

/// Ensure a market exists with given configuration and optionally adds to queues and supply
pub fn ensure_market(
    c: &mut crate::Contract,
    id: AccountId,
    cap: u128,
    enabled: bool,
    supply: u128,
    in_withdraw: bool,
    in_supply: bool,
    removable_at: u64,
) {
    let mut cfg = templar_common::vault::MarketConfiguration::default();
    cfg.cap = near_sdk::json_types::U128(cap);
    cfg.enabled = enabled;
    cfg.removable_at = removable_at;
    c.config.insert(id.clone(), cfg);
    if supply > 0 {
        c.market_supply.insert(id.clone(), supply);
    }
    if in_withdraw && !c.withdraw_queue.iter().any(|m| m == &id) {
        c.withdraw_queue.push(id.clone());
    }
    if in_supply && !c.supply_queue.iter().any(|m| m == &id) {
        c.supply_queue.push(id.clone());
    }
}

/// Seed shares into the vault's own account (used for escrow/burn tests)
pub fn seed_vault_shares(c: &mut crate::Contract, shares: u128) {
    #[allow(clippy::expect_used, reason = "test helper")]
    c.deposit_unchecked(&near_sdk::env::current_account_id(), shares)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
}
