use crate::{aux::ReturnStyle, Contract, ContractExt, OpState};
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{env, json_types::U128, near, require, AccountId, PromiseOrValue};
use templar_common::vault::DepositMsg;

#[allow(clippy::wildcard_imports)]
use near_sdk_contract_tools::mt::*;

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
        const RETURN_STYLE: ReturnStyle = ReturnStyle::Nep141FtTransferCall;

        let msg = near_sdk::serde_json::from_str::<DepositMsg>(&msg)
            .unwrap_or_else(|_| env::panic_str("Invalid deposit msg"));

        let asset_id = env::predecessor_account_id();

        match msg {
            DepositMsg::Supply => {
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
        sender_id: AccountId,
        previous_owner_ids: Vec<AccountId>,
        token_ids: Vec<TokenId>,
        amounts: Vec<U128>,
        msg: String,
    ) -> PromiseOrValue<Vec<U128>> {
        const RETURN_STYLE: ReturnStyle = ReturnStyle::Nep245MtTransferCall;

        // NEP-245: This could be an authorized account ID. We only care about
        // the actual previous owner.
        let _ = sender_id;

        let msg = near_sdk::serde_json::from_str::<DepositMsg>(&msg)
            .unwrap_or_else(|_| env::panic_str("Invalid deposit msg"));

        require!(
            token_ids.len() == 1,
            "This contract only accepts one token at a time."
        );
        require!(
            previous_owner_ids.len() == 1 && amounts.len() == 1,
            "Invalid input length"
        );

        let token_id = &token_ids[0];
        let sender_id = previous_owner_ids[0].clone();
        let amount = amounts[0];

        match msg {
            DepositMsg::Supply => {
                let refund = self.execute_supply(
                    sender_id.clone(),
                    // FIXME: this is incorrect, we should abstract this into the underlying to
                    // determine the kind.
                    token_id
                        .parse()
                        .unwrap_or_else(|_| env::panic_str("Invalid token ID")),
                    amount.into(),
                );

                PromiseOrValue::Value(vec![U128(refund)])
            }
        }
    }
}

impl Contract {
    pub(crate) fn execute_supply(
        &mut self,
        sender_id: AccountId,
        token_id: AccountId,
        deposit: u128,
    ) -> u128 {
        // Invariant: Only the underlying token is accepted; others are fully refunded
        if token_id != self.underlying_asset.contract_id() {
            env::log_str("Only underlying asset is supported");
            return deposit;
        };

        if deposit == 0 {
            env::log_str("Deposit is zero");
            return 0;
        }

        self.internal_accrue_fee();

        let max = self.get_max_deposit().0;
        let accept = deposit.min(max);
        let refund = deposit - accept;

        let shares = self.preview_deposit(U128(accept)).0;
        self.mint_shares(&sender_id, shares);

        self.idle_balance = self.idle_balance.saturating_add(accept);
        self.last_total_assets = self.last_total_assets.saturating_add(accept);

        if matches!(self.op_state, OpState::Idle) {
            // Invariant: no overlapping operations
            env::log_str("Starting allocation");
            self.start_allocation(self.idle_balance);
        }

        refund
    }
}
