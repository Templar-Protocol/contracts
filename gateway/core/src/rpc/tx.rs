use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::write_method_spec,
    rpc::common::{ContractArgs, WriteOperationResult},
    ContractMethodName, NearGas, NearToken,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FunctionCallBody {
    pub receiver_id: AccountId,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
    pub gas: NearGas,
    pub deposit: NearToken,
}

pub type FunctionCallResult = WriteOperationResult;

write_method_spec!(
    FunctionCall,
    "tx.functionCall",
    FunctionCallBody,
    FunctionCallResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferNep141Body {
    pub token_id: AccountId,
    pub receiver_id: AccountId,
    pub amount: crate::U128,
}

pub type TransferNep141Result = WriteOperationResult;

write_method_spec!(
    TransferNep141,
    "tx.transferNep141",
    TransferNep141Body,
    TransferNep141Result
);
