use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::write_method_spec,
    rpc::common::{ContractArgs, WriteOperationResult},
    ContractMethodName, GenericWriteMethod, NearGas, NearToken, WriteMethod,
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
    WriteMethod::Generic(GenericWriteMethod::FunctionCall),
    FunctionCallBody,
    FunctionCallResult
);
