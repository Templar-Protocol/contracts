#![allow(clippy::all)]

use std::collections::HashMap;

use crate::Contract;
use near_sdk::NearToken;
pub use near_sdk::{
    test_utils::VMContextBuilder, test_vm_config, testing_env, AccountId, PromiseResult,
    RuntimeFeesConfig,
};
use near_sdk_contract_tools::ft::Nep145;
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
        HashMap::default(),
        promise_results
    );
}

pub fn new_test_contract(vault_id: &AccountId) -> Contract {
    setup_env(vault_id, vault_id, vec![]);

    // Basic accounts
    let owner = mk(1);
    let curator = mk(2);
    let guardian = mk(3);
    let sentinel = mk(7);
    let fee_recipient = mk(4);
    let skim_recipient = mk(5);
    let underlying_token_id = mk(6);

    let cfg = vault_configuration(
        owner.clone(),
        curator.clone(),
        guardian.clone(),
        sentinel.clone(),
        underlying_token_id.clone(),
        skim_recipient.clone(),
        fee_recipient.clone(),
    );

    let mut builder = VMContextBuilder::new();
    builder.current_account_id(vault_id.clone());
    builder.predecessor_account_id(vault_id.clone());
    builder.signer_account_id(vault_id.clone());
    builder.attached_deposit(NearToken::from_near(1));
    testing_env!(
        builder.build(),
        test_vm_config(),
        RuntimeFeesConfig::test(),
        HashMap::default(),
        vec![]
    );
    let mut c = Contract::new(cfg);
    c.storage_deposit(Some(owner), None);
    c.storage_deposit(Some(curator), None);
    c.storage_deposit(Some(guardian), None);
    c.storage_deposit(Some(sentinel), None);
    c.storage_deposit(Some(fee_recipient), None);
    c.storage_deposit(Some(skim_recipient), None);
    c.storage_deposit(Some(underlying_token_id), None);

    setup_env(vault_id, vault_id, vec![]);
    c
}
/// Set the block timestamp and keep caller/predecessor consistent for tests
pub fn set_block_ts(vault_id: &AccountId, signer: &AccountId, ts: u64) {
    set_ctx(vault_id, signer, Some(ts), None);
}

pub fn set_ctx(vault_id: &AccountId, signer: &AccountId, ts: Option<u64>, deposit: Option<u128>) {
    set_ctx_with_gas(vault_id, signer, ts, deposit, None);
}

pub fn set_ctx_with_gas(
    vault_id: &AccountId,
    signer: &AccountId,
    ts: Option<u64>,
    deposit: Option<u128>,
    prepaid_gas: Option<near_sdk::Gas>,
) {
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
    if let Some(gas) = prepaid_gas {
        ctx.prepaid_gas(gas);
    }
    testing_env!(ctx.build());
}
