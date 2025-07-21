use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use near_primitives::{
    action::{Action, delegate::SignedDelegateAction},
    types::{AccountId, Gas},
};
use near_sdk::serde::Deserialize;
use near_sdk::serde_json;
use templar_common::{
    asset::{BorrowAsset, CollateralAsset, FungibleAsset},
    market::DepositMsg,
};
use templar_relayer::{FtTransferCallArgs, GasDescriptors, MtTransferCallArgs, TransferCallArgs};

#[derive(Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
struct Configuration {
    pub database_url: String,
    pub rpc_url: String,
    pub gas_descriptors_path: PathBuf,
}

struct MarketAccounts {
    pub account_id: AccountId,
    pub collateral_asset: FungibleAsset<CollateralAsset>,
    pub borrow_asset: FungibleAsset<BorrowAsset>,
}

struct App {
    pub configuration: Configuration,
    pub gas_descriptors: GasDescriptors,
    pub market_account_ids: HashMap<AccountId, MarketAccounts>,
    pub allowed_receiver_account_ids: HashSet<AccountId>,
}

#[derive(Debug, thiserror::Error)]
#[error("Failed precondition: ")]
pub enum PreconditionError {
    #[error("Failed signature verification")]
    SignatureVerificationFailure,
    #[error("Unknown transaction receiver account ID {account_id}")]
    UnknownTransactionReceiverId { account_id: AccountId },
    #[error("Unsupported action at index {index}")]
    UnsupportedAction { index: usize },
    #[error("Argument deserialization failure at index {index}")]
    ArgumentDeserializationFailure { index: usize },
    #[error("Msg deserialization failure at index {index}")]
    MsgDeserializationFailure { index: usize },
    #[error("Unknown token transfer receiver account ID {account_id} at index {index}")]
    UnknownTransferReceiverId { account_id: AccountId, index: usize },
    #[error("Invalid message for asset at index {index}")]
    InvalidMsgForAsset { index: usize },
    #[error("Unknown function name `{name}` at index {index}")]
    UnknownFunctionName { name: String, index: usize },
}

impl App {
    pub fn calculate_gas(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<Gas, PreconditionError> {
        if !signed_delegate_action.verify() {
            return Err(PreconditionError::SignatureVerificationFailure);
        }

        let receiver_id = &signed_delegate_action.delegate_action.receiver_id;

        if !self.allowed_receiver_account_ids.contains(receiver_id) {
            return Err(PreconditionError::UnknownTransactionReceiverId {
                account_id: receiver_id.clone(),
            });
        }

        let actions = signed_delegate_action.delegate_action.get_actions();
        let len = actions.len();
        let calls = actions
            .into_iter()
            .try_fold(Vec::with_capacity(len), |mut v, action| {
                if let Action::FunctionCall(fc) = action {
                    v.push(fc);
                    return Ok(v);
                } else {
                    return Err(v.len());
                }
            })
            .map_err(|index| PreconditionError::UnsupportedAction { index })?;

        if self.market_account_ids.contains_key(receiver_id) {
            // Calling a market contract directly.
            let method_names = HashSet::from([
                "borrow",
                "apply_interest",
                "harvest_yield",
                "withdraw_static_yield",
                "withdraw_collateral",
                "create_supply_withdrawal_request",
                "execute_next_supply_withdrawal_request",
            ]);
            for (index, call) in calls.iter().enumerate() {
                if !method_names.contains(call.method_name.as_str()) {
                    return Err(PreconditionError::UnknownFunctionName {
                        name: call.method_name.to_owned(),
                        index,
                    });
                }
            }
        } else {
            // Token contract transfer call to market.
            for (index, call) in calls.iter().enumerate() {
                fn deserialize_args<'de, T: Deserialize<'de>>(
                    slice: &'de [u8],
                    index: usize,
                ) -> Result<T, PreconditionError> {
                    serde_json::from_slice::<T>(slice)
                        .map_err(|_| PreconditionError::ArgumentDeserializationFailure { index })
                }

                let (args, asset) = match &call.method_name[..] {
                    "ft_transfer_call" => {
                        let args = deserialize_args::<FtTransferCallArgs>(&call.args, index)?;

                        (
                            Box::new(args) as Box<dyn TransferCallArgs>,
                            FungibleAsset::<BorrowAsset>::nep141(receiver_id.clone()),
                        )
                    }
                    "mt_transfer_call" => {
                        let args = deserialize_args::<MtTransferCallArgs>(&call.args, index)?;
                        let token_id = args.token_id.clone();

                        (
                            Box::new(args) as Box<dyn TransferCallArgs>,
                            FungibleAsset::<BorrowAsset>::nep245(receiver_id.clone(), token_id),
                        )
                    }
                    name => {
                        return Err(PreconditionError::UnknownFunctionName {
                            name: name.to_owned(),
                            index,
                        });
                    }
                };

                let market_id = args.receiver_id();

                let Some(market_account_ids) = self.market_account_ids.get(market_id) else {
                    return Err(PreconditionError::UnknownTransferReceiverId {
                        account_id: market_id.to_owned(),
                        index,
                    });
                };

                let Ok(msg) = serde_json::from_str::<DepositMsg>(args.msg()) else {
                    return Err(PreconditionError::MsgDeserializationFailure { index });
                };

                if market_account_ids.borrow_asset == asset {
                    match msg {
                        DepositMsg::Supply | DepositMsg::Repay => { /* ok */ }
                        _ => return Err(PreconditionError::InvalidMsgForAsset { index }),
                    }
                } else if market_account_ids.collateral_asset == asset.coerce() {
                    match msg {
                        DepositMsg::Collateralize => { /* ok */ }
                        _ => return Err(PreconditionError::InvalidMsgForAsset { index }),
                    }
                } else {
                    return Err(PreconditionError::UnknownTransactionReceiverId {
                        account_id: receiver_id.clone(),
                    });
                }
            }
        }

        Ok(calls.iter().map(|call| call.gas).sum())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();
    let configuration: Configuration = envy::from_env().unwrap();
}
