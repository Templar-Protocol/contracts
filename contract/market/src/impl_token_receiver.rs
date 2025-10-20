use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{env, json_types::U128, near, require, AccountId, PromiseOrValue};
#[allow(clippy::wildcard_imports)]
use near_sdk_contract_tools::mt::*;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount, ReturnStyle},
    market::DepositMsg,
    self_ext,
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
        const RETURN_STYLE: ReturnStyle = ReturnStyle::Nep141FtTransferCall;

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
                        .price_oracle_configuration
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_COLLATERALIZE_TRANSFER_CALL_01_CONSUME_PRICE)
                                .collateralize_transfer_call_01_consume_price(
                                    sender_id,
                                    amount,
                                    RETURN_STYLE,
                                ),
                        ),
                )
            }
            DepositMsg::Repay => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .price_oracle_configuration
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_REPAY_TRANSFER_CALL_01_CONSUME_PRICE)
                                .repay_transfer_call_01_consume_price(
                                    sender_id,
                                    amount,
                                    RETURN_STYLE,
                                ),
                        ),
                )
            }
            DepositMsg::Liquidate(msg) => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .price_oracle_configuration
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_LIQUIDATE_TRANSFER_CALL_01_CONSUME_PRICE)
                                .liquidate_transfer_call_01_consume_price(
                                    sender_id,
                                    amount,
                                    msg,
                                    RETURN_STYLE,
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
        const RETURN_STYLE: ReturnStyle = ReturnStyle::Nep245MtTransferCall;

        // NEP-245: This could be an authorized account ID. We only care about
        // the actual previous owner.
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
        let sender_id = previous_owner_ids[0].clone();
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

                self.execute_supply(sender_id, amount);

                PromiseOrValue::Value(vec![U128(0)])
            }
            DepositMsg::Collateralize => {
                let amount = use_collateral_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .price_oracle_configuration
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_COLLATERALIZE_TRANSFER_CALL_01_CONSUME_PRICE)
                                .collateralize_transfer_call_01_consume_price(
                                    sender_id,
                                    amount,
                                    RETURN_STYLE,
                                ),
                        ),
                )
            }
            DepositMsg::Repay => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .price_oracle_configuration
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_REPAY_TRANSFER_CALL_01_CONSUME_PRICE)
                                .repay_transfer_call_01_consume_price(
                                    sender_id,
                                    amount,
                                    RETURN_STYLE,
                                ),
                        ),
                )
            }
            DepositMsg::Liquidate(msg) => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .price_oracle_configuration
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_LIQUIDATE_TRANSFER_CALL_01_CONSUME_PRICE)
                                .liquidate_transfer_call_01_consume_price(
                                    sender_id,
                                    amount,
                                    msg,
                                    RETURN_STYLE,
                                ),
                        ),
                )
            }
        }
    }
}
