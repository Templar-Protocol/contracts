use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::public_read_method_spec,
    rpc::common::{ContractArgs, TxExecutionStatus},
    ChainReadMethod, ContractMethodName, CryptoHash, NearGas, NearToken, PublicReadMethod,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ViewAccountParams {
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ViewAccountResult {
    pub amount: NearToken,
    pub locked: NearToken,
    pub code_hash: String,
    pub storage_usage: u64,
    pub global_contract_hash: Option<String>,
    pub global_contract_account_id: Option<AccountId>,
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewFunctionResult {
    pub value: serde_json::Value,
}

public_read_method_spec!(
    ViewFunction,
    "chain.viewFunction",
    PublicReadMethod::Chain(ChainReadMethod::ViewFunction),
    ViewFunctionParams,
    ViewFunctionResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTransactionParams {
    /// Transaction hash to query.
    pub tx_hash: CryptoHash,
    /// Original signer account for the transaction.
    pub sender_account_id: AccountId,
    /// Desired execution status / finality depth for the query.
    ///
    /// If omitted, the gateway queries at `FINAL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until: Option<TxExecutionStatus>,
    /// Preferred decoding for the execution return value.
    ///
    /// Even for successful transactions, the return value may be absent.
    #[serde(default)]
    pub encoding: ValueEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ValueEncoding {
    #[default]
    Json,
    Base64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub enum TransactionReturnValue {
    Json(serde_json::Value),
    Base64(crate::Base64Bytes),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStatus {
    Pending,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GetTransactionResult {
    /// Gateway-level summary of the transaction execution state.
    pub status: TransactionStatus,
    /// Total gas burnt across the execution outcome observed by the query.
    pub total_gas_burnt: NearGas,
    /// Logs emitted by the execution outcomes included in the queried result.
    ///
    /// Successful transactions may still return an empty log list.
    pub logs: Vec<String>,
    /// Decoded execution return value, if one exists.
    ///
    /// Successful transactions may not produce return bytes, so this field is optional.
    pub return_value: Option<TransactionReturnValue>,
}

public_read_method_spec!(
    GetTransaction,
    "chain.getTransaction",
    PublicReadMethod::Chain(ChainReadMethod::GetTransaction),
    GetTransactionParams,
    GetTransactionResult
);
