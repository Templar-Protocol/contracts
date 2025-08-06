use near_fetch::signer::KeyRotatingSigner;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{delegate::SignedDelegateAction, Action},
    types::Finality,
};
use near_sdk::{serde_json::json, AccountId};
use templar_common::market::MarketConfiguration;

use crate::MarketAccounts;

#[derive(Clone)]
pub struct NearClient {
    client: near_fetch::Client,
    signer: KeyRotatingSigner,
}

impl std::fmt::Debug for NearClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NearClient")
            .field("client", &self.client)
            .field("signer", &"[hidden]")
            .finish()
    }
}

#[allow(clippy::unwrap_used)]
impl NearClient {
    pub fn new(client: JsonRpcClient, signer: KeyRotatingSigner) -> Self {
        let client = near_fetch::Client::from_client(client);
        Self { client, signer }
    }

    /// # Errors
    ///
    /// - When there is an error sending the transaction.
    pub async fn sign_and_send(
        &self,
        signed_delegate_action: SignedDelegateAction,
    ) -> Result<near_primitives::views::FinalExecutionOutcomeView, near_fetch::Error> {
        let delegate_receiver_id = signed_delegate_action.delegate_action.sender_id.clone();
        let actions = vec![Action::Delegate(Box::new(signed_delegate_action))];
        self.client
            .send_tx(&self.signer, &delegate_receiver_id, actions)
            .await
    }

    pub async fn load_deployments_from_registry(&self, registry_id: &AccountId) -> Vec<AccountId> {
        self.client
            .view(registry_id, "list_deployments")
            .args_json(json!({}))
            .finality(Finality::Final)
            .await
            .unwrap()
            .json::<Vec<AccountId>>()
            .unwrap()
    }

    pub async fn load_market_accounts(&self, market_id: &AccountId) -> Option<MarketAccounts> {
        let market_configuration = self
            .client
            .view(market_id, "get_configuration")
            .args_json(json!({}))
            .finality(Finality::Final)
            .await
            .ok()?
            .json::<MarketConfiguration>()
            .ok()?;

        Some(MarketAccounts {
            account_id: market_id.clone(),
            borrow_asset: market_configuration.borrow_asset,
            collateral_asset: market_configuration.collateral_asset,
        })
    }
}
