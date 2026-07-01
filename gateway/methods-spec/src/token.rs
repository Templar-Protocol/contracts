use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::asset::{AssetClass, FungibleAsset};
use templar_gateway_macros::MethodSpec;
use templar_primitives::SU128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "standard", rename_all = "snake_case")]
pub enum TokenReference {
    Ft {
        contract_id: AccountId,
    },
    Mt {
        contract_id: AccountId,
        token_id: String,
    },
}

impl<T: AssetClass> From<&FungibleAsset<T>> for TokenReference {
    fn from(asset: &FungibleAsset<T>) -> Self {
        let contract_id = asset.contract_id().to_owned();
        match asset.nep245_token_id() {
            Some(token_id) => Self::Mt {
                contract_id,
                token_id: token_id.to_owned(),
            },
            None => Self::Ft { contract_id },
        }
    }
}

/// Get a token balance across supported standards.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "token.getBalanceOf", output = GetBalanceOfResult)]
pub struct GetBalanceOf {
    pub token: TokenReference,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: SU128,
}

/// Transfer a token across supported standards.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "token.transfer")]
pub struct Transfer {
    pub token: TokenReference,
    pub receiver_id: AccountId,
    pub amount: SU128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// Transfer a token and call the receiver.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "token.transferCall")]
pub struct TransferCall {
    pub token: TokenReference,
    pub receiver_id: AccountId,
    pub amount: SU128,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}
