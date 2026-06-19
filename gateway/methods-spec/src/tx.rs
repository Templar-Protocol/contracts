use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::{read_method_spec, write_method_spec};
use templar_gateway_types::{
    common::{ContractArgs, TxExecutionStatus},
    Base64Bytes, ContractMethodName, CryptoHash, NearGas, NearToken,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Get {
    pub tx_hash: CryptoHash,
    pub sender_account_id: AccountId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until: Option<TxExecutionStatus>,
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
pub enum ReturnValue {
    Json(serde_json::Value),
    Base64(Base64Bytes),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GetResult {
    pub status: Status,
    pub total_gas_burnt: NearGas,
    pub logs: Vec<String>,
    pub return_value: Option<ReturnValue>,
}

read_method_spec!(
    /// Fetch transaction execution status and result details.
    "tx.get": Get -> GetResult
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FunctionCall {
    pub receiver_id: AccountId,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
    pub gas: NearGas,
    pub deposit: NearToken,
}

write_method_spec!(
    /// Submit a single function-call transaction.
    "tx.functionCall": FunctionCall
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Transfer {
    pub receiver_id: AccountId,
    pub amount: NearToken,
}

write_method_spec!(
    /// Transfer native NEAR to another account.
    "tx.transfer": Transfer
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeployContract {
    pub account_id: AccountId,
    pub code: Base64Bytes,
}

write_method_spec!(
    /// Deploy contract code to an existing account in a single transaction.
    "tx.deployContract": DeployContract
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeployAndInit {
    pub account_id: AccountId,
    pub code: Base64Bytes,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
    pub gas: NearGas,
    pub deposit: NearToken,
}

write_method_spec!(
    /// Deploy contract code and call its init method in one transaction.
    "tx.deployAndInit": DeployAndInit
);
