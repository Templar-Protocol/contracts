use std::collections::{HashMap, HashSet};

use near_primitives::{action::FunctionCallAction, types::AccountId};
use near_sdk::{
    json_types::U128,
    serde::{Deserialize, Serialize},
    serde_json, AccountIdRef,
};

use near_sdk_contract_tools::standard::nep145::StorageBalanceBounds;
use templar_common::{
    asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAsset},
    oracle::{pyth::PriceIdentifier, OracleRequest},
};

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
    pub allowed_methods: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct MarketData {
    pub account_id: AccountId,
    pub oracle_id: AccountId,
    pub collateral: AssetResolution<CollateralAsset>,
    pub borrow: AssetResolution<BorrowAsset>,
}

#[derive(Debug, Clone)]
pub struct AssetResolution<A: AssetClass> {
    pub asset: FungibleAsset<A>,
    pub price_id: PriceIdentifier,
    pub update_oracle: OracleRequest,
}

pub struct AssetTransfer {
    pub args: TransferCallArgs,
    pub receiver_id: AccountId,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AssetTransferParseError {
    #[error("Unknown function name")]
    UnknownFunctionName,
    #[error("Failed to deserialize arguments")]
    ArgumentDeserialization,
}

impl AssetTransfer {
    /// # Errors
    ///
    /// - Argument deserialization
    /// - Unknown function name
    pub fn parse(
        receiver_id: AccountId,
        call: &FunctionCallAction,
    ) -> Result<Self, AssetTransferParseError> {
        let args = match &call.method_name[..] {
            "ft_transfer_call" => TransferCallArgs::Nep141(
                serde_json::from_slice(&call.args)
                    .map_err(|_| AssetTransferParseError::ArgumentDeserialization)?,
            ),
            "mt_transfer_call" => TransferCallArgs::Nep245(
                serde_json::from_slice(&call.args)
                    .map_err(|_| AssetTransferParseError::ArgumentDeserialization)?,
            ),
            _ => return Err(AssetTransferParseError::UnknownFunctionName),
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
