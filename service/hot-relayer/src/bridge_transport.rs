use async_trait::async_trait;

use crate::hot_relayer::{
    deposit_sign_request_from_event_checked, plan_stellar_withdraw_execution_checked,
    DepositSignRequest, HotMpcSigner, HotRelayerError, HotRelayerRouting, PendingWithdrawal,
    StellarDepositEvent, StellarWithdrawExecution,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositCompletion {
    pub sign_request: DepositSignRequest,
    pub signature: String,
}

#[async_trait]
pub trait BridgeRelayer {
    async fn complete_deposit(
        &self,
        event: &StellarDepositEvent,
    ) -> Result<DepositCompletion, HotRelayerError>;

    async fn complete_withdrawal(
        &self,
        pending: &PendingWithdrawal,
    ) -> Result<StellarWithdrawExecution, HotRelayerError>;
}

#[derive(Debug, Clone)]
pub struct HotBridgeRelayer<S> {
    routing: HotRelayerRouting,
    signer: S,
}

impl<S> HotBridgeRelayer<S> {
    #[must_use]
    pub fn new(routing: HotRelayerRouting, signer: S) -> Self {
        Self { routing, signer }
    }

    #[must_use]
    pub fn routing(&self) -> &HotRelayerRouting {
        &self.routing
    }
}

#[async_trait]
impl<S> BridgeRelayer for HotBridgeRelayer<S>
where
    S: HotMpcSigner + Send + Sync,
{
    async fn complete_deposit(
        &self,
        event: &StellarDepositEvent,
    ) -> Result<DepositCompletion, HotRelayerError> {
        let sign_request = deposit_sign_request_from_event_checked(event, &self.routing)?;
        let signature = self.signer.deposit_sign(&sign_request).await?;
        Ok(DepositCompletion {
            sign_request,
            signature,
        })
    }

    async fn complete_withdrawal(
        &self,
        pending: &PendingWithdrawal,
    ) -> Result<StellarWithdrawExecution, HotRelayerError> {
        plan_stellar_withdraw_execution_checked(&self.signer, pending, &self.routing).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct RecordingSigner {
        deposit_calls: Mutex<Vec<DepositSignRequest>>,
    }

    #[async_trait]
    impl HotMpcSigner for RecordingSigner {
        async fn withdraw_sign(&self, nonce: &str) -> Result<String, HotRelayerError> {
            Ok(format!("sig-withdraw-{nonce}"))
        }

        async fn deposit_sign(
            &self,
            request: &DepositSignRequest,
        ) -> Result<String, HotRelayerError> {
            self.deposit_calls
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .push(request.clone());
            Ok("sig-deposit".to_string())
        }
    }

    fn routing() -> HotRelayerRouting {
        HotRelayerRouting::new(
            "vault-counterparty.near".to_string(),
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            1100,
            "1100_CUSDC".to_string(),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[tokio::test]
    async fn hot_relayer_completes_deposit_through_checked_route() {
        let signer = RecordingSigner::default();
        let relayer = HotBridgeRelayer::new(routing(), signer);
        let event = StellarDepositEvent {
            chain_id: 1100,
            nonce: "55".to_string(),
            sender_id: "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            receiver_id: "vault-counterparty.near".to_string(),
            token_id: "1100_CUSDC".to_string(),
            amount: "9".to_string(),
        };

        let completion = relayer
            .complete_deposit(&event)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(completion.signature, "sig-deposit");
        assert_eq!(
            completion.sign_request.receiver_id,
            "vault-counterparty.near"
        );
    }

    #[tokio::test]
    async fn hot_relayer_rejects_unexpected_deposit_receiver() {
        let signer = RecordingSigner::default();
        let relayer = HotBridgeRelayer::new(routing(), signer);
        let event = StellarDepositEvent {
            chain_id: 1100,
            nonce: "55".to_string(),
            sender_id: "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            receiver_id: "unexpected.near".to_string(),
            token_id: "1100_CUSDC".to_string(),
            amount: "9".to_string(),
        };

        let error = relayer
            .complete_deposit(&event)
            .await
            .expect_err("expected receiver mismatch");
        assert!(matches!(
            error,
            HotRelayerError::UnexpectedReceiver {
                direction: "deposit",
                ..
            }
        ));
    }
}
