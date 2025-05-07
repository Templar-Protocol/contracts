use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{env, json_types::U128, near, AccountId, PromiseOrValue};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    market::{LiquidateMsg, Nep141MarketDepositMessage},
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
        let msg = near_sdk::serde_json::from_str::<Nep141MarketDepositMessage>(&msg)
            .unwrap_or_else(|_| env::panic_str("Invalid ft_on_transfer msg"));

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
            Nep141MarketDepositMessage::Supply => {
                let amount = use_borrow_asset();

                self.execute_supply(sender_id, amount);

                PromiseOrValue::Value(U128(0))
            }
            Nep141MarketDepositMessage::Collateralize => {
                let amount = use_collateral_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_FT_ON_TRANSFER_COLLATERALIZE_01_CONSUME_PRICE)
                                .ft_on_transfer_collateralize_01_consume_price(sender_id, amount),
                        ),
                )
            }
            Nep141MarketDepositMessage::Repay => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_FT_ON_TRANSFER_REPAY_01_CONSUME_PRICE)
                                .ft_on_transfer_repay_01_consume_price(sender_id, amount),
                        ),
                )
            }
            Nep141MarketDepositMessage::Liquidate(LiquidateMsg { account_id }) => {
                let amount = use_borrow_asset();

                PromiseOrValue::Promise(
                    self.configuration
                        .balance_oracle
                        .retrieve_price_pair()
                        .then(
                            self_ext!(Self::GAS_FT_ON_TRANSFER_LIQUIDATE_01_CONSUME_PRICE)
                                .ft_on_transfer_liquidate_01_consume_price(
                                    sender_id, account_id, amount,
                                ),
                        ),
                )
            }
        }
    }
}
