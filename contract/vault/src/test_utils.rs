use crate::Contract;
use near_sdk::{
    test_utils::{accounts, VMContextBuilder},
    test_vm_config, testing_env, AccountId, PromiseResult, RuntimeFeesConfig,
};
use rstest::rstest;
use templar_common::vault::{AllocationMode, OpState, VaultConfiguration};
use test_utils::vault_configuration;

pub fn mk(n: u32) -> AccountId {
    format!("acc{n}.testnet").parse().expect("valid account id")
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
