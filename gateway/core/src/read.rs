use async_trait::async_trait;
use near_api::types::{
    account::Account,
    transaction::{actions::AccessKey, result::ExecutionFinalResult},
    CryptoHash, PublicKey, TxExecutionStatus,
};
use near_api::{Account as NearAccountView, Contract, Transaction};
use serde::de::DeserializeOwned;
use templar_gateway_types::Base64Bytes;

use crate::{GatewayError, GatewayResult, NearClient};

#[async_trait]
pub trait ReadNear: Send + Sync {
    async fn view_function<T>(
        &self,
        contract_id: near_account_id::AccountId,
        method_name: &str,
        args: Vec<u8>,
    ) -> GatewayResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static;

    async fn view_account(&self, account_id: near_account_id::AccountId) -> GatewayResult<Account>;

    async fn view_access_key(
        &self,
        account_id: near_account_id::AccountId,
        public_key: PublicKey,
    ) -> GatewayResult<AccessKey>;

    async fn view_contract_code(
        &self,
        account_id: near_account_id::AccountId,
    ) -> GatewayResult<Base64Bytes>;

    async fn view_contract_state(
        &self,
        account_id: near_account_id::AccountId,
        prefix: Vec<u8>,
    ) -> GatewayResult<Vec<(Base64Bytes, Base64Bytes)>>;

    async fn view_transaction_status(
        &self,
        sender_account_id: near_account_id::AccountId,
        tx_hash: CryptoHash,
        wait_until: TxExecutionStatus,
    ) -> GatewayResult<ExecutionFinalResult>;
}

#[async_trait]
impl ReadNear for NearClient {
    async fn view_function<T>(
        &self,
        contract_id: near_account_id::AccountId,
        method_name: &str,
        args: Vec<u8>,
    ) -> GatewayResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static,
    {
        Contract(contract_id.clone())
            .call_function_raw(method_name, args)
            .read_only()
            .at(self.finality_policy().query_reference())
            .fetch_from(self.network())
            .await
            .map(|response| response.data)
            .map_err(|error| account_query_error(contract_id, error))
    }

    async fn view_account(&self, account_id: near_account_id::AccountId) -> GatewayResult<Account> {
        let account = NearAccountView(account_id.clone())
            .view()
            .at(self.finality_policy().query_reference())
            .fetch_from(self.network())
            .await
            .map_err(|error| account_query_error(account_id.clone(), error))?;
        Ok(account.data)
    }

    async fn view_access_key(
        &self,
        account_id: near_account_id::AccountId,
        public_key: PublicKey,
    ) -> GatewayResult<AccessKey> {
        let key = NearAccountView(account_id.clone())
            .access_key(public_key)
            .at(self.finality_policy().query_reference())
            .fetch_from(self.network())
            .await
            .map_err(|error| account_query_error(account_id.clone(), error))?;
        Ok(key.data)
    }

    async fn view_contract_code(
        &self,
        account_id: near_account_id::AccountId,
    ) -> GatewayResult<Base64Bytes> {
        Contract(account_id.clone())
            .wasm()
            .at(self.finality_policy().query_reference())
            .fetch_from(self.network())
            .await
            .map_err(|error| account_query_error(account_id, error))
            .and_then(|response| decode_base64(response.data.code_base64, "contract code"))
    }

    async fn view_contract_state(
        &self,
        account_id: near_account_id::AccountId,
        prefix: Vec<u8>,
    ) -> GatewayResult<Vec<(Base64Bytes, Base64Bytes)>> {
        Contract(account_id.clone())
            .view_storage_with_prefix(&prefix)
            .at(self.finality_policy().query_reference())
            .fetch_from(self.network())
            .await
            .map_err(|error| account_query_error(account_id, error))
            .and_then(|response| {
                response
                    .data
                    .values
                    .into_iter()
                    .map(|entry| {
                        Ok((
                            decode_base64(entry.key.0, "storage key")?,
                            decode_base64(entry.value.0, "storage value")?,
                        ))
                    })
                    .collect()
            })
    }

    async fn view_transaction_status(
        &self,
        sender_account_id: near_account_id::AccountId,
        tx_hash: CryptoHash,
        wait_until: TxExecutionStatus,
    ) -> GatewayResult<ExecutionFinalResult> {
        Transaction::status_with_options(sender_account_id, tx_hash, wait_until)
            .fetch_from(self.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))
    }
}

fn decode_base64(value: String, what: &str) -> GatewayResult<Base64Bytes> {
    serde_json::from_value(serde_json::Value::String(value))
        .map_err(|error| GatewayError::NearQuery(format!("decode {what}: {error}")))
}

fn account_query_error<E: std::fmt::Debug + std::fmt::Display>(
    account_id: near_account_id::AccountId,
    error: E,
) -> GatewayError {
    if is_unknown_account(&error) {
        GatewayError::AccountNotFound(account_id)
    } else {
        GatewayError::NearQuery(error.to_string())
    }
}

/// Whether a view error means the account does not exist (as opposed to a
/// transient query failure). The node surfaces this inconsistently — sometimes
/// as a typed `UnknownAccount` query error, sometimes as a plain message (see
/// near-api's own note about message-form RPC errors) — so match the stable RPC
/// error name in the rendered error to catch both forms.
fn is_unknown_account<E: std::fmt::Debug>(error: &E) -> bool {
    let rendered = format!("{error:?}");
    rendered.contains("UnknownAccount") || rendered.contains("UNKNOWN_ACCOUNT")
}

/// Whether a transaction-status error means the chain has no record of the
/// transaction (`UnknownTransaction`), as opposed to a transient/transport
/// failure or a still-pending `TimeoutError`. Matched on the rendered error for
/// the same reason as [`is_unknown_account`].
pub(crate) fn is_unknown_transaction<E: std::fmt::Debug>(error: &E) -> bool {
    let rendered = format!("{error:?}");
    rendered.contains("UnknownTransaction") || rendered.contains("UNKNOWN_TRANSACTION")
}

#[cfg(test)]
mod tests {
    use super::is_unknown_account;

    // `&str`'s `Debug` renders its contents, standing in for a real error's
    // rendered form without constructing the deeply nested near-api error types.
    #[test]
    fn detects_unknown_account_in_both_error_forms() {
        // Typed query-error form (Rust variant name in the Debug output).
        assert!(is_unknown_account(
            &"ServerError(UnknownAccount { requested_account_id: alice.near })"
        ));
        // Message form the node sometimes returns instead of a typed object.
        assert!(is_unknown_account(&"handler error: UNKNOWN_ACCOUNT"));
        // Unrelated failures must not be mistaken for non-existence.
        assert!(!is_unknown_account(&"TransportError(connection timed out)"));
        assert!(!is_unknown_account(
            &"ServerError(MethodNotFound { method_name: foo })"
        ));
    }

    #[test]
    fn detects_unknown_transaction_but_not_transient_errors() {
        assert!(super::is_unknown_transaction(
            &"ServerError(UnknownTransaction { requested_transaction_hash: 11..11 })"
        ));
        assert!(super::is_unknown_transaction(
            &"handler error: UNKNOWN_TRANSACTION"
        ));
        // A still-pending or unreachable transaction must NOT look terminal.
        assert!(!super::is_unknown_transaction(&"ServerError(TimeoutError)"));
        assert!(!super::is_unknown_transaction(
            &"TransportError(connection timed out)"
        ));
    }
}
