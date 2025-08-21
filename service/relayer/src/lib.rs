use std::collections::HashMap;

use near_primitives::{action::FunctionCallAction, types::AccountId};
use near_sdk::{
    json_types::U128,
    serde::{Deserialize, Serialize},
    serde_json, AccountIdRef,
};

use near_sdk_contract_tools::standard::nep145::StorageBalanceBounds;
use templar_common::asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAsset};

use error::PreconditionError;

pub mod app;
pub mod broom;
pub mod cache;
pub mod client;
pub mod error;
pub mod route;

#[derive(Debug, Clone, Default)]
pub struct AccountData {
    pub market_data: HashMap<AccountId, MarketData>,
    pub allowed_contract_data: HashMap<AccountId, ContractData>,
}

#[derive(Debug, Clone)]
pub struct ContractData {
    pub storage_balance_bounds: Option<StorageBalanceBounds>,
}

#[derive(Debug, Clone)]
pub struct MarketData {
    pub account_id: AccountId,
    pub collateral_asset: FungibleAsset<CollateralAsset>,
    pub borrow_asset: FungibleAsset<BorrowAsset>,
}

pub struct AssetTransfer {
    pub args: TransferCallArgs,
    pub receiver_id: AccountId,
}

impl AssetTransfer {
    /// # Errors
    ///
    /// - Argument deserialization
    /// - Unknown function name
    pub fn parse(
        call: &FunctionCallAction,
        index: usize,
        receiver_id: AccountId,
    ) -> Result<Self, PreconditionError> {
        let args = match &call.method_name[..] {
            "ft_transfer_call" => TransferCallArgs::Nep141(deserialize_args(&call.args, index)?),
            "mt_transfer_call" => TransferCallArgs::Nep245(deserialize_args(&call.args, index)?),
            name => {
                return Err(PreconditionError::UnknownFunctionName {
                    name: name.to_owned(),
                    index,
                })
            }
        };

        Ok(Self { args, receiver_id })
    }

    pub fn asset<T: AssetClass>(&self) -> FungibleAsset<T> {
        match &self.args {
            TransferCallArgs::Nep141(_) => FungibleAsset::nep141(self.receiver_id.clone()),
            TransferCallArgs::Nep245(args) => {
                FungibleAsset::nep245(self.receiver_id.clone(), args.token_id.clone())
            }
        }
    }

    pub fn contract_id(&self) -> &AccountIdRef {
        &self.receiver_id
    }

    pub fn token_receiver_id(&self) -> &AccountIdRef {
        self.args.receiver_id()
    }
}

pub enum TransferCallArgs {
    Nep141(FtTransferCallArgs),
    Nep245(MtTransferCallArgs),
}

impl TransferCallArgs {
    pub fn receiver_id(&self) -> &AccountIdRef {
        match self {
            TransferCallArgs::Nep141(args) => &args.receiver_id,
            TransferCallArgs::Nep245(args) => &args.receiver_id,
        }
    }

    pub fn msg(&self) -> &str {
        match self {
            TransferCallArgs::Nep141(args) => &args.msg,
            TransferCallArgs::Nep245(args) => &args.msg,
        }
    }
}

fn deserialize_args<'de, T: Deserialize<'de>>(
    slice: &'de [u8],
    index: usize,
) -> Result<T, PreconditionError> {
    serde_json::from_slice::<T>(slice)
        .map_err(|_| PreconditionError::ArgumentDeserializationFailure { index })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct FtTransferCallArgs {
    pub receiver_id: AccountId,
    pub amount: U128,
    pub memo: Option<String>,
    pub msg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct MtTransferCallArgs {
    pub receiver_id: AccountId,
    pub token_id: String,
    pub amount: U128,
    pub memo: Option<String>,
    pub msg: String,
}
