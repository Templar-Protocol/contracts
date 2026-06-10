use crate::convert::account_id_to_address;
use crate::kernel_effects::{apply_kernel_effects, KernelEffectContext};
use crate::{Contract, ContractExt, OpState};
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{env, json_types::U128, near, require, AccountId, PromiseOrValue};
use templar_common::vault::{require_at_least, DepositMsg, IdleBalanceDelta, SUPPLY_GAS};
use templar_vault_kernel::actions::apply_action;
use templar_vault_kernel::effects::KernelEffect;
use templar_vault_kernel::KernelAction;
use templar_vault_kernel::TimestampNs;

use near_sdk_contract_tools::mt::{Nep245Receiver, TokenId};

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

        if self.idle_resync_inflight_op_id != 0 {
            templar_common::panic_with_message("Cannot deposit during idle resync");
        }

        self.internal_accrue_fee();

        let max = self.get_max_deposit().0;
        let accept = deposit.min(max);
        if accept == 0 {
            return deposit;
        }

        let kernel_state = self.kernel_state_mirror();
        let kernel_config = self.kernel_config_mirror();
        let kernel_restrictions = self.kernel_restrictions_mirror();
        let sender_address = account_id_to_address(&sender_id);
        let self_address = account_id_to_address(&env::current_account_id());

        let result = apply_action(
            kernel_state,
            &kernel_config,
            kernel_restrictions.as_ref(),
            &self_address,
            KernelAction::Deposit {
                owner: sender_address,
                receiver: sender_address,
                assets_in: accept,
                min_shares_out: 0,
                now_ns: TimestampNs(env::block_timestamp()),
            },
        )
        .unwrap_or_else(|_| templar_common::panic_with_message("Kernel deposit failed"));

        let shares_out = result
            .effects
            .iter()
            .find_map(|effect| match effect {
                KernelEffect::MintShares { shares, .. } => Some(*shares),
                _ => None,
            })
            .unwrap_or(0);

        if shares_out == 0 {
            return deposit;
        }

        let mut ctx = KernelEffectContext::default();
        ctx.insert(sender_address, sender_id.clone());

        apply_kernel_effects(self, &result.effects, &ctx).unwrap_or_else(|_| {
            templar_common::panic_with_message("Failed to apply kernel deposit effects")
        });

        self.update_idle_balance(IdleBalanceDelta::Increase(accept.into()));
        self.fee_anchor.total_assets = U128(result.state.total_assets);
        self.fee_anchor.timestamp_ns = env::block_timestamp().into();

        deposit - accept
    }
}
