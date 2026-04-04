use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::public_read_method_spec,
    rpc::common::{ContractArgs, JsonValueResult},
    ChainReadMethod, ContractMethodName, PublicReadMethod,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ViewAccountParams {
    pub account_id: AccountId,
}

pub type ViewAccountResult = JsonValueResult;

public_read_method_spec!(
    ViewAccount,
    "chain.viewAccount",
    PublicReadMethod::Chain(ChainReadMethod::ViewAccount),
    ViewAccountParams,
    ViewAccountResult
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewFunctionParams {
    pub contract_id: AccountId,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
}

pub type ViewFunctionResult = JsonValueResult;

public_read_method_spec!(
    ViewFunction,
    "chain.viewFunction",
    PublicReadMethod::Chain(ChainReadMethod::ViewFunction),
    ViewFunctionParams,
    ViewFunctionResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTransactionParams {
    pub tx_hash: String,
    pub sender_account_id: AccountId,
}

pub type GetTransactionResult = JsonValueResult;

public_read_method_spec!(
    GetTransaction,
    "chain.getTransaction",
    PublicReadMethod::Chain(ChainReadMethod::GetTransaction),
    GetTransactionParams,
    GetTransactionResult
);
