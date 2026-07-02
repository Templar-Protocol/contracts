use async_trait::async_trait;
use near_api::types::transaction::actions::{
    AccessKeyPermission as NearAccessKeyPermission, Action, DeleteAccountAction,
};
use templar_gateway_core::{
    GatewayError, {DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite},
};
use templar_gateway_methods_spec::account;
use templar_gateway_types::ContractMethodName;

use crate::Dispatch;

/// Translate near_api's access-key permission into the gateway spec's typed
/// permission, parsing the function-call `receiver_id` into an `AccountId` and
/// wrapping method names so the public schema stays strongly typed.
fn permission_view(
    permission: NearAccessKeyPermission,
) -> GatewayResult<account::AccessKeyPermission> {
    Ok(match permission {
        NearAccessKeyPermission::FullAccess => account::AccessKeyPermission::FullAccess,
        NearAccessKeyPermission::FunctionCall(function_call) => {
            account::AccessKeyPermission::FunctionCall {
                allowance: function_call.allowance,
                receiver_id: function_call.receiver_id.parse().map_err(|error| {
                    GatewayError::NearQuery(format!(
                        "access key has an invalid function-call receiver_id: {error}"
                    ))
                })?,
                method_names: function_call
                    .method_names
                    .into_iter()
                    .map(ContractMethodName::from)
                    .collect(),
            }
        }
    })
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<account::Get, C> for Dispatch {
    async fn dispatch(request: account::Get, ctx: C) -> GatewayResult<account::GetResult> {
        let account = ctx.near_client().account().get(request.account_id).await?;

        let (code_hash, global_contract_hash, global_contract_account_id) =
            match account.contract_state {
                near_api::types::account::ContractState::LocalHash(hash) => {
                    (hash.to_string(), None, None)
                }
                near_api::types::account::ContractState::GlobalHash(hash) => (
                    near_api::types::CryptoHash::default().to_string(),
                    Some(hash.to_string()),
                    None,
                ),
                near_api::types::account::ContractState::GlobalAccountId(account_id) => (
                    near_api::types::CryptoHash::default().to_string(),
                    None,
                    Some(account_id),
                ),
                near_api::types::account::ContractState::None => (
                    near_api::types::CryptoHash::default().to_string(),
                    None,
                    None,
                ),
            };

        Ok(account::GetResult {
            amount: account.amount,
            locked: account.locked,
            code_hash,
            storage_usage: account.storage_usage,
            global_contract_hash,
            global_contract_account_id,
        })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<account::GetAccessKey, C> for Dispatch {
    async fn dispatch(
        request: account::GetAccessKey,
        ctx: C,
    ) -> GatewayResult<account::GetAccessKeyResult> {
        let key = ctx
            .near_client()
            .account()
            .access_key(request.account_id, request.public_key.into())
            .await?;

        Ok(account::GetAccessKeyResult {
            nonce: key.nonce.0,
            permission: permission_view(key.permission)?,
        })
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<account::Delete, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<account::Delete>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::execute(
            request.signer_account_id.clone(),
            request.signer_account_id.0,
            vec![Action::DeleteAccount(DeleteAccountAction {
                beneficiary_id: request.body.beneficiary_id,
            })],
        ))
    }
}

#[cfg(test)]
mod tests {
    use near_api::types::transaction::actions::{
        AccessKeyPermission as NearAccessKeyPermission, FunctionCallPermission,
    };
    use near_api::types::NearToken;
    use templar_gateway_methods_spec::account;

    use super::permission_view;

    #[test]
    fn full_access_maps_through() {
        assert!(matches!(
            permission_view(NearAccessKeyPermission::FullAccess).unwrap(),
            account::AccessKeyPermission::FullAccess
        ));
    }

    #[test]
    fn function_call_carries_all_fields() {
        let permission = NearAccessKeyPermission::FunctionCall(FunctionCallPermission {
            allowance: Some(NearToken::from_near(2)),
            receiver_id: "market.near".to_owned(),
            method_names: vec!["borrow".to_owned(), "repay".to_owned()],
        });

        match permission_view(permission).unwrap() {
            account::AccessKeyPermission::FunctionCall {
                allowance,
                receiver_id,
                method_names,
            } => {
                assert_eq!(allowance, Some(NearToken::from_near(2)));
                assert_eq!(receiver_id.as_str(), "market.near");
                assert_eq!(
                    method_names
                        .iter()
                        .map(|m| m.0.as_str())
                        .collect::<Vec<_>>(),
                    ["borrow", "repay"]
                );
            }
            account::AccessKeyPermission::FullAccess => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn function_call_rejects_invalid_receiver_id() {
        let permission = NearAccessKeyPermission::FunctionCall(FunctionCallPermission {
            allowance: None,
            receiver_id: "NOT a valid account".to_owned(),
            method_names: vec![],
        });
        assert!(permission_view(permission).is_err());
    }
}
