use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{env, json_types::U128, near, require, AccountId, PromiseOrValue};
#[allow(clippy::wildcard_imports)]
use near_sdk_contract_tools::mt::*;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    market::{DepositMsg, LiquidateMsg},
};

use crate::{Contract, ContractExt};

#[near]
impl FungibleTokenReceiver for Contract {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let msg = near_sdk::serde_json::from_str::<DepositMsg>(&msg)
            .unwrap_or_else(|_| env::panic_str("Invalid deposit msg"));

        let asset_id = env::predecessor_account_id();

        let use_borrow_asset = || {
            if !self.configuration.borrow_asset.is_nep141(&asset_id) {
                env::panic_str("Unsupported borrow asset");
            }

            BorrowAssetAmount::new(amount.0)
        };

        let use_collateral_asset = || {
            if !self.configuration.collateral_asset.is_nep141(&asset_id) {
                env::panic_str("Unsupported collateral asset");
            }

            CollateralAssetAmount::new(amount.0)
        };

        match msg {
            DepositMsg::Supply => {
                let amount = use_borrow_asset();

                self.execute_supply(sender_id, amount);

                PromiseOrValue::Value(U128(0))
            }
            DepositMsg::Collateralize => {
                let amount = use_collateral_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_ON_TRANSFER_COLLATERALIZE_01_CONSUME_PRICE)
                                .on_transfer_collateralize_01_consume_price(
                                    sender_id, amount, false,
                                ),
                        ),
                )
            }
            DepositMsg::Repay => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_ON_TRANSFER_REPAY_01_CONSUME_PRICE)
                                .on_transfer_repay_01_consume_price(sender_id, amount, false),
                        ),
                )
            }
            DepositMsg::Liquidate(LiquidateMsg { account_id }) => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_LIQUIDATE_TRANSFER_CALL_01_CONSUME_ORACLE_RESPONSE)
                                .liquidate_transfer_call_01_consume_oracle_response(
                                    sender_id, account_id, amount, false,
                                ),
                        ),
                )
            }
        }
    }
}

#[near]
impl Nep245Receiver for Contract {
    fn mt_on_transfer(
        &mut self,
        sender_id: AccountId,
        previous_owner_ids: Vec<AccountId>,
        token_ids: Vec<TokenId>,
        amounts: Vec<U128>,
        msg: String,
    ) -> PromiseOrValue<Vec<U128>> {
        let _ = sender_id;

        let msg = near_sdk::serde_json::from_str::<DepositMsg>(&msg)
            .unwrap_or_else(|_| env::panic_str("Invalid deposit msg"));

        let contract_id = env::predecessor_account_id();

        require!(
            token_ids.len() == 1,
            "This contract only accepts one token at a time."
        );
        require!(
            previous_owner_ids.len() == 1 && amounts.len() == 1,
            "Invalid input length"
        );

        let token_id = &token_ids[0];
        let sender_id = &previous_owner_ids[0];
        let amount = amounts[0];

        let use_borrow_asset = || {
            if !self
                .configuration
                .borrow_asset
                .is_nep245(&contract_id, token_id)
            {
                env::panic_str("Unsupported borrow asset");
            }

            BorrowAssetAmount::new(amount.0)
        };

        let use_collateral_asset = || {
            if !self
                .configuration
                .collateral_asset
                .is_nep245(&contract_id, token_id)
            {
                env::panic_str("Unsupported collateral asset");
            }

            CollateralAssetAmount::new(amount.0)
        };

        match msg {
            DepositMsg::Supply => {
                let amount = use_borrow_asset();

                self.execute_supply(sender_id.clone(), amount);

                PromiseOrValue::Value(vec![U128(0)])
            }
            DepositMsg::Collateralize => {
                let amount = use_collateral_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_ON_TRANSFER_COLLATERALIZE_01_CONSUME_PRICE)
                                .on_transfer_collateralize_01_consume_price(
                                    sender_id.clone(),
                                    amount,
                                    true,
                                ),
                        ),
                )
            }
            DepositMsg::Repay => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_ON_TRANSFER_REPAY_01_CONSUME_PRICE)
                                .on_transfer_repay_01_consume_price(
                                    sender_id.clone(),
                                    amount,
                                    true,
                                ),
                        ),
                )
            }
            DepositMsg::Liquidate(LiquidateMsg { account_id }) => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_LIQUIDATE_TRANSFER_CALL_01_CONSUME_ORACLE_RESPONSE)
                                .liquidate_transfer_call_01_consume_oracle_response(
                                    sender_id.clone(),
                                    account_id,
                                    amount,
                                    true,
                                ),
                        ),
                )
            }
        }
    }
}
