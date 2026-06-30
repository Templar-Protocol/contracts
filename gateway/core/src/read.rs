use async_trait::async_trait;
use near_api::types::{
    account::Account,
    transaction::{actions::AccessKey, result::ExecutionFinalResult},
    CryptoHash, PublicKey, TxExecutionStatus,
};
use near_api::{Account as NearAccountView, Contract, Transaction};
use serde::de::DeserializeOwned;

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
        Ok(Contract(contract_id)
            .call_function_raw(method_name, args)
            .read_only()
            .fetch_from(self.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?
            .data)
    }

    async fn view_account(&self, account_id: near_account_id::AccountId) -> GatewayResult<Account> {
        let account = NearAccountView(account_id.clone())
            .view()
            .fetch_from(self.network())
            .await
            .map_err(|error| {
                if is_unknown_account(&error) {
                    GatewayError::AccountNotFound(account_id)
                } else {
                    GatewayError::NearQuery(error.to_string())
                }
            })?;
        Ok(account.data)
    }

    async fn view_access_key(
        &self,
        account_id: near_account_id::AccountId,
        public_key: PublicKey,
    ) -> GatewayResult<AccessKey> {
        let key = NearAccountView(account_id)
            .access_key(public_key)
            .fetch_from(self.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        Ok(key.data)
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
