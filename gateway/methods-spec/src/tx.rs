use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{
    common::{ContractArgs, TxExecutionStatus},
    Base64Bytes, ContractMethodName, CryptoHash, NearGas, NearToken,
};

/// Fetch transaction execution status and result details.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "tx.get", output = GetResult)]
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
    /// Total NEAR burnt across the transaction and all its receipts — the
    /// actual cost the signer paid (not always `gas × gas_price`).
    pub tokens_burnt: NearToken,
    pub logs: Vec<String>,
    pub return_value: Option<ReturnValue>,
    /// Accounts whose receipts failed, even when the top-level transaction
    /// succeeded. NEAR reports `status` from the final receipt only, so an
    /// `ft_transfer_call` whose receiver callback panicked (and was refunded by
    /// `ft_resolve_transfer`) still shows `Succeeded` here; a consumer that
    /// requires every receipt to have succeeded must check this is empty.
    pub failed_receipts: Vec<AccountId>,
}

/// Submit a single function-call transaction.
#[derive(MethodSpec, Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[method(write = "tx.functionCall")]
pub struct FunctionCall {
    pub receiver_id: AccountId,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
    pub gas: NearGas,
    pub deposit: NearToken,
}

/// Transfer native NEAR to another account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "tx.transfer")]
pub struct Transfer {
    pub receiver_id: AccountId,
    pub amount: NearToken,
}

/// Relay a NEP-366 signed delegate action (meta-transaction): the signing
/// account submits a transaction carrying the delegate action and pays its gas.
/// `signed_delegate_action` is the borsh-encoded `SignedDelegateAction`.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "tx.relaySignedDelegateAction")]
pub struct RelaySignedDelegateAction {
    pub signed_delegate_action: Base64Bytes,
}

/// Deploy contract code to an existing account in a single transaction.
#[derive(MethodSpec, Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[method(write = "tx.deployContract")]
pub struct DeployContract {
    pub account_id: AccountId,
    pub code: Base64Bytes,
}

/// Deploy contract code and call its init method in one transaction.
#[derive(MethodSpec, Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[method(write = "tx.deployAndInit")]
pub struct DeployAndInit {
    pub account_id: AccountId,
    pub code: Base64Bytes,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
    pub gas: NearGas,
    pub deposit: NearToken,
}
