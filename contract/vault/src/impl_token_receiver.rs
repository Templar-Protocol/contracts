use crate::{Contract, ContractExt, OpState};
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{env, json_types::U128, near, require, AccountId, PromiseOrValue};
use near_sdk_contract_tools::ft::{Nep141Controller as _, Nep141Mint};
use templar_common::vault::{require_at_least, DepositMsg, Event, IdleBalanceDelta, SUPPLY_GAS};

#[allow(clippy::wildcard_imports)]
use near_sdk_contract_tools::mt::*;

// Parses JSON-encoded DepositMsg or panics with a consistent message.
fn parse_deposit_msg(msg: &str) -> DepositMsg {
    near_sdk::serde_json::from_str(msg)
        .unwrap_or_else(|_| templar_common::panic_with_message("Invalid deposit msg"))
}

// Validates NEP-245 transfer inputs and returns (depositor, token_id, amount).
fn validate_single_mt_input<'a>(
    previous_owner_ids: &'a [AccountId],
    token_ids: &'a [TokenId],
    amounts: &'a [U128],
) -> (AccountId, &'a TokenId, U128) {
    require!(
        token_ids.len() == 1,
        "This contract only accepts one token at a time."
    );
    require!(
        previous_owner_ids.len() == 1 && amounts.len() == 1,
        "Invalid input length"
    );
    let depositor = previous_owner_ids[0].clone();
    let token_id = &token_ids[0];
    let amount = amounts[0];
    (depositor, token_id, amount)
}

#[near]
impl FungibleTokenReceiver for Contract {
    /// NEP-141 token receiver for deposits.
    /// Expects a JSON-encoded `DepositMsg` in `msg` (currently only `Supply` is supported).
    /// Returns the unused amount to refund to the sender as required by NEP-141.
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let msg = parse_deposit_msg(&msg);

        let asset_id = env::predecessor_account_id();

        match msg {
            DepositMsg::Supply => {
                require_at_least(SUPPLY_GAS);

                self.gate.enforce_policy(&sender_id);

                let refund = self.execute_supply(sender_id, asset_id, amount.into());
                PromiseOrValue::Value(refund.into())
            }
        }
    }
}

#[near]
impl Nep245Receiver for Contract {
    /// NEP-245 multi-token receiver for deposits.
    /// Only accepts a single token ID and amount. The token ID must match the underlying asset.
    /// Returns a one-element vector with the unused amount to refund to the sender.
    fn mt_on_transfer(
        &mut self,
        #[allow(clippy::used_underscore_binding)] _sender_id: AccountId,
        previous_owner_ids: Vec<AccountId>,
        token_ids: Vec<TokenId>,
        amounts: Vec<U128>,
        msg: String,
    ) -> PromiseOrValue<Vec<U128>> {
        let msg = parse_deposit_msg(&msg);

        let (sender_id, token_id, amount) =
            validate_single_mt_input(&previous_owner_ids, &token_ids, &amounts);

        match msg {
            DepositMsg::Supply => {
                require_at_least(SUPPLY_GAS);

                self.gate.enforce_policy(&sender_id);

                let token_contract = env::predecessor_account_id();

                require!(
                    self.underlying_asset.is_nep245(&token_contract, token_id),
                    "Invalid token ID"
                );

                let refund = self.execute_supply(sender_id.clone(), token_contract, amount.into());

                PromiseOrValue::Value(vec![U128(refund)])
            }
        }
    }
}

impl Contract {
    pub(crate) fn execute_supply(
        &mut self,
        sender_id: AccountId,
        asset_id: AccountId,
        deposit: u128,
    ) -> u128 {
        // Invariant: Only the underlying token is accepted; others are fully refunded
        require!(
            asset_id == self.underlying_asset.contract_id(),
            "Invalid token ID"
        );

        require!(deposit > 0, "Deposit amount must be greater than zero");

        if matches!(self.op_state, OpState::Payout(_)) {
            templar_common::panic_with_message("Cannot deposit during payout");
        }

        self.internal_accrue_fee();

        let max = self.get_max_deposit().0;
        let accept = deposit.min(max);
        let refund = deposit - accept;

        let shares = self.preview_deposit(U128(accept)).0;
        self.mint(&Nep141Mint::new(shares, &sender_id))
            .unwrap_or_else(|_| templar_common::panic_with_message("Failed to mint shares"));

        Event::MintedShares {
            amount: shares.into(),
            receiver: sender_id.clone(),
        }
        .emit();

        self.update_idle_balance(IdleBalanceDelta::Increase(accept.into()));
        self.last_total_assets = self.last_total_assets.saturating_add(accept);

        refund
    }
}
