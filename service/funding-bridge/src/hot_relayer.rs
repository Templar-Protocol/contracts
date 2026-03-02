//! HOT Bridge relayer primitives used to verify receiver-safety assumptions.
//!
//! This module focuses on one invariant:
//! - For withdrawals, relayer asks MPC to sign by `nonce` only, then uses
//!   receiver data already committed on NEAR (`withdraw_data.receiver_id`).
//! - For deposits, relayer signs exactly the tuple extracted from the source
//!   chain event, including `receiver_id`.

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingWithdrawal {
    pub nonce: String,
    pub chain_id: u64,
    pub withdraw_data: PendingWithdrawData,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingWithdrawData {
    pub receiver_id: String,
    pub amount: String,
    pub token_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StellarDepositEvent {
    pub chain_id: u64,
    pub nonce: String,
    pub sender_id: String,
    pub receiver_id: String,
    pub token_id: String,
    pub amount: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DepositSignRequest {
    pub chain: u64,
    pub nonce: String,
    pub sender_id: String,
    pub receiver_id: String,
    pub token_id: String,
    pub amount: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autopilot: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StellarWithdrawExecution {
    pub nonce: String,
    pub token_id: String,
    pub receiver: String,
    pub amount: String,
    pub signature: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HotRelayerError {
    #[error("http request failed: {0}")]
    Http(String),
    #[error("mpc api returned status {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("failed to decode mpc response: {0}")]
    Decode(String),
    #[error("unexpected {direction} receiver: expected {expected}, got {actual}")]
    UnexpectedReceiver {
        direction: &'static str,
        expected: String,
        actual: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
struct SignatureResponse {
    signature: String,
}

#[async_trait]
pub trait HotMpcSigner {
    async fn withdraw_sign(&self, nonce: &str) -> Result<String, HotRelayerError>;
    async fn deposit_sign(&self, request: &DepositSignRequest) -> Result<String, HotRelayerError>;
}

#[derive(Debug, Clone)]
pub struct HotMpcApiClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotRelayerRouting {
    pub near_receiver: String,
    pub stellar_receiver: String,
}

impl HotRelayerRouting {
    pub fn validate_deposit_event(
        &self,
        event: &StellarDepositEvent,
    ) -> Result<(), HotRelayerError> {
        if event.receiver_id != self.near_receiver {
            return Err(HotRelayerError::UnexpectedReceiver {
                direction: "deposit",
                expected: self.near_receiver.clone(),
                actual: event.receiver_id.clone(),
            });
        }
        Ok(())
    }

    pub fn validate_pending_withdrawal(
        &self,
        pending: &PendingWithdrawal,
    ) -> Result<(), HotRelayerError> {
        if pending.withdraw_data.receiver_id != self.stellar_receiver {
            return Err(HotRelayerError::UnexpectedReceiver {
                direction: "withdrawal",
                expected: self.stellar_receiver.clone(),
                actual: pending.withdraw_data.receiver_id.clone(),
            });
        }
        Ok(())
    }
}

impl HotMpcApiClient {
    #[must_use]
    pub fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    async fn sign<T: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<String, HotRelayerError> {
        let response = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|e| HotRelayerError::Http(e.to_string()))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| HotRelayerError::Http(e.to_string()))?;
        let body_text = String::from_utf8_lossy(&bytes).to_string();

        if status != StatusCode::OK {
            return Err(HotRelayerError::HttpStatus {
                status: status.as_u16(),
                body: body_text,
            });
        }

        let parsed: SignatureResponse =
            serde_json::from_slice(&bytes).map_err(|e| HotRelayerError::Decode(e.to_string()))?;
        Ok(parsed.signature)
    }
}

#[async_trait]
impl HotMpcSigner for HotMpcApiClient {
    async fn withdraw_sign(&self, nonce: &str) -> Result<String, HotRelayerError> {
        #[derive(Serialize)]
        struct WithdrawSignRequest<'a> {
            nonce: &'a str,
        }

        self.sign("/withdraw/sign", &WithdrawSignRequest { nonce })
            .await
    }

    async fn deposit_sign(&self, request: &DepositSignRequest) -> Result<String, HotRelayerError> {
        self.sign("/deposit/sign", request).await
    }
}

#[must_use]
fn deposit_sign_request_from_event_unchecked(event: &StellarDepositEvent) -> DepositSignRequest {
    DepositSignRequest {
        chain: event.chain_id,
        nonce: event.nonce.clone(),
        sender_id: event.sender_id.clone(),
        receiver_id: event.receiver_id.clone(),
        token_id: event.token_id.clone(),
        amount: event.amount.clone(),
        autopilot: None,
    }
}

pub fn deposit_sign_request_from_event_checked(
    event: &StellarDepositEvent,
    routing: &HotRelayerRouting,
) -> Result<DepositSignRequest, HotRelayerError> {
    routing.validate_deposit_event(event)?;
    Ok(deposit_sign_request_from_event_unchecked(event))
}

#[must_use]
pub fn build_stellar_withdraw_execution(
    pending: &PendingWithdrawal,
    signature: String,
) -> StellarWithdrawExecution {
    StellarWithdrawExecution {
        nonce: pending.nonce.clone(),
        token_id: pending.withdraw_data.token_id.clone(),
        receiver: pending.withdraw_data.receiver_id.clone(),
        amount: pending.withdraw_data.amount.clone(),
        signature,
    }
}

async fn plan_stellar_withdraw_execution_unchecked<S: HotMpcSigner + Sync>(
    signer: &S,
    pending: &PendingWithdrawal,
) -> Result<StellarWithdrawExecution, HotRelayerError> {
    let signature = signer.withdraw_sign(&pending.nonce).await?;
    Ok(build_stellar_withdraw_execution(pending, signature))
}

pub async fn plan_stellar_withdraw_execution_checked<S: HotMpcSigner + Sync>(
    signer: &S,
    pending: &PendingWithdrawal,
    routing: &HotRelayerRouting,
) -> Result<StellarWithdrawExecution, HotRelayerError> {
    routing.validate_pending_withdrawal(pending)?;
    plan_stellar_withdraw_execution_unchecked(signer, pending).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;
    use wiremock::{
        matchers::{body_json, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingSigner {
        withdraw_nonces: Mutex<Vec<String>>,
        deposit_requests: Mutex<Vec<DepositSignRequest>>,
    }

    #[async_trait]
    impl HotMpcSigner for RecordingSigner {
        async fn withdraw_sign(&self, nonce: &str) -> Result<String, HotRelayerError> {
            self.withdraw_nonces
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .push(nonce.to_string());
            Ok("withdraw-signature".to_string())
        }

        async fn deposit_sign(
            &self,
            request: &DepositSignRequest,
        ) -> Result<String, HotRelayerError> {
            self.deposit_requests
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .push(request.clone());
            Ok("deposit-signature".to_string())
        }
    }

    #[tokio::test]
    async fn withdrawal_plan_uses_nonce_for_sign_and_committed_receiver_for_execution() {
        let signer = RecordingSigner::default();
        let pending = PendingWithdrawal {
            nonce: "991".to_string(),
            chain_id: 1100,
            withdraw_data: PendingWithdrawData {
                receiver_id: "GADAPTERADDRESS".to_string(),
                amount: "1500".to_string(),
                token_id: "1100_CUSDC".to_string(),
            },
        };

        let routing = HotRelayerRouting {
            near_receiver: "vault-counterparty.near".to_string(),
            stellar_receiver: "GADAPTERADDRESS".to_string(),
        };

        let execution = plan_stellar_withdraw_execution_checked(&signer, &pending, &routing)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(execution.nonce, "991");
        assert_eq!(execution.signature, "withdraw-signature");
        assert_eq!(execution.receiver, "GADAPTERADDRESS");
        assert_eq!(execution.token_id, "1100_CUSDC");
        assert_eq!(execution.amount, "1500");
        assert_eq!(
            signer
                .withdraw_nonces
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .as_slice(),
            &["991".to_string()]
        );
    }

    #[test]
    fn deposit_sign_request_copies_receiver_from_event() {
        let event = StellarDepositEvent {
            chain_id: 1100,
            nonce: "21".to_string(),
            sender_id: "GVAULT".to_string(),
            receiver_id: "vault-counterparty.near".to_string(),
            token_id: "1100_CUSDC".to_string(),
            amount: "42".to_string(),
        };
        let routing = HotRelayerRouting {
            near_receiver: "vault-counterparty.near".to_string(),
            stellar_receiver: "GADAPTERADDRESS".to_string(),
        };

        let request = deposit_sign_request_from_event_checked(&event, &routing)
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(request.chain, 1100);
        assert_eq!(request.nonce, "21");
        assert_eq!(request.sender_id, "GVAULT");
        assert_eq!(request.receiver_id, "vault-counterparty.near");
        assert_eq!(request.token_id, "1100_CUSDC");
        assert_eq!(request.amount, "42");
        assert_eq!(request.autopilot, None);
    }

    #[test]
    fn deposit_sign_request_checked_rejects_unexpected_receiver() {
        let event = StellarDepositEvent {
            chain_id: 1100,
            nonce: "21".to_string(),
            sender_id: "GVAULT".to_string(),
            receiver_id: "wrong.near".to_string(),
            token_id: "1100_CUSDC".to_string(),
            amount: "42".to_string(),
        };
        let routing = HotRelayerRouting {
            near_receiver: "vault-counterparty.near".to_string(),
            stellar_receiver: "GADAPTERADDRESS".to_string(),
        };

        let error = deposit_sign_request_from_event_checked(&event, &routing)
            .expect_err("expected receiver mismatch");
        assert!(matches!(
            error,
            HotRelayerError::UnexpectedReceiver {
                direction: "deposit",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn withdrawal_plan_checked_rejects_unexpected_receiver() {
        let signer = RecordingSigner::default();
        let pending = PendingWithdrawal {
            nonce: "991".to_string(),
            chain_id: 1100,
            withdraw_data: PendingWithdrawData {
                receiver_id: "wrong-address".to_string(),
                amount: "1500".to_string(),
                token_id: "1100_CUSDC".to_string(),
            },
        };
        let routing = HotRelayerRouting {
            near_receiver: "vault-counterparty.near".to_string(),
            stellar_receiver: "GADAPTERADDRESS".to_string(),
        };

        let error = plan_stellar_withdraw_execution_checked(&signer, &pending, &routing)
            .await
            .expect_err("expected receiver mismatch");
        assert!(matches!(
            error,
            HotRelayerError::UnexpectedReceiver {
                direction: "withdrawal",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn mpc_client_posts_only_nonce_for_withdraw_sign() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/withdraw/sign"))
            .and(body_json(json!({ "nonce": "4242" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "signature": "sig-1"
            })))
            .mount(&server)
            .await;

        let client = HotMpcApiClient::new(server.uri());
        let signature = client
            .withdraw_sign("4242")
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(signature, "sig-1");
    }

    #[tokio::test]
    async fn mpc_client_posts_deposit_sign_tuple_including_receiver() {
        let server = MockServer::start().await;
        let request = DepositSignRequest {
            chain: 1100,
            nonce: "12".to_string(),
            sender_id: "GVAULT".to_string(),
            receiver_id: "vault-counterparty.near".to_string(),
            token_id: "1100_CUSDC".to_string(),
            amount: "77".to_string(),
            autopilot: None,
        };

        Mock::given(method("POST"))
            .and(path("/deposit/sign"))
            .and(body_json(json!({
                "chain": 1100,
                "nonce": "12",
                "sender_id": "GVAULT",
                "receiver_id": "vault-counterparty.near",
                "token_id": "1100_CUSDC",
                "amount": "77"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "signature": "sig-2"
            })))
            .mount(&server)
            .await;

        let client = HotMpcApiClient::new(server.uri());
        let signature = client
            .deposit_sign(&request)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(signature, "sig-2");
    }
}
