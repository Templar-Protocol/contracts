//! HOT Bridge relayer primitives used to verify receiver-safety assumptions.
//!
//! This module focuses on one invariant:
//! - For withdrawals, relayer asks MPC to sign by `nonce` only, then uses
//!   receiver data already committed on NEAR (`withdraw_data.receiver_id`).
//! - For deposits, relayer signs exactly the tuple extracted from the source
//!   chain event, including `receiver_id`.

use async_trait::async_trait;
use getset::{CopyGetters, Getters};
use near_primitives::types::AccountId;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::{str::FromStr, time::Duration};
use stellar_xdr::curr::ScAddress;

use crate::config::HotMpcApiUrl;

pub const HOT_STELLAR_CHAIN_ID: u64 = 1100;

const MAX_HOT_NONCE_LEN: usize = 64;
const MAX_HOT_TOKEN_ID_LEN: usize = 256;
const MAX_HOT_AMOUNT_LEN: usize = 64;

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
    #[error("invalid {direction} {field}: {reason}")]
    InvalidField {
        direction: &'static str,
        field: &'static str,
        reason: String,
    },
    #[error("invalid HOT relayer routing config {field}: {reason}")]
    InvalidRouting { field: &'static str, reason: String },
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
    base_url: HotMpcApiUrl,
}

#[derive(Debug, Clone, Getters, CopyGetters, PartialEq, Eq)]
pub struct HotRelayerRouting {
    #[get = "pub"]
    near_receiver: String,
    #[get = "pub"]
    stellar_receiver: String,
    #[get_copy = "pub"]
    chain_id: u64,
    #[get = "pub"]
    token_id: String,
}

impl HotRelayerRouting {
    pub fn new(
        near_receiver: String,
        stellar_receiver: String,
        chain_id: u64,
        token_id: String,
    ) -> Result<Self, HotRelayerError> {
        validate_near_receiver(&near_receiver).map_err(|reason| {
            HotRelayerError::InvalidRouting {
                field: "near_receiver",
                reason,
            }
        })?;
        validate_stellar_receiver(&stellar_receiver).map_err(|reason| {
            HotRelayerError::InvalidRouting {
                field: "stellar_receiver",
                reason,
            }
        })?;
        validate_chain_id(chain_id).map_err(|reason| HotRelayerError::InvalidRouting {
            field: "chain_id",
            reason,
        })?;
        validate_token_id(&token_id, chain_id).map_err(|reason| {
            HotRelayerError::InvalidRouting {
                field: "token_id",
                reason,
            }
        })?;

        Ok(Self {
            near_receiver,
            stellar_receiver,
            chain_id,
            token_id,
        })
    }

    pub fn validate_deposit_event(
        &self,
        event: &StellarDepositEvent,
    ) -> Result<(), HotRelayerError> {
        validate_chain_id_matches("deposit", event.chain_id, self.chain_id)?;
        validate_nonce("deposit", &event.nonce)?;
        validate_stellar_account("deposit", "sender_id", &event.sender_id)?;
        validate_token_id_matches("deposit", &event.token_id, &self.token_id, self.chain_id)?;
        validate_amount("deposit", &event.amount)?;

        if event.receiver_id != self.near_receiver {
            return Err(HotRelayerError::UnexpectedReceiver {
                direction: "deposit",
                expected: self.near_receiver.clone(),
                actual: event.receiver_id.clone(),
            });
        }
        validate_near_account("deposit", "receiver_id", &event.receiver_id)?;
        Ok(())
    }

    pub fn validate_pending_withdrawal(
        &self,
        pending: &PendingWithdrawal,
    ) -> Result<(), HotRelayerError> {
        validate_chain_id_matches("withdrawal", pending.chain_id, self.chain_id)?;
        validate_nonce("withdrawal", &pending.nonce)?;
        validate_token_id_matches(
            "withdrawal",
            &pending.withdraw_data.token_id,
            &self.token_id,
            self.chain_id,
        )?;
        validate_amount("withdrawal", &pending.withdraw_data.amount)?;

        if pending.withdraw_data.receiver_id != self.stellar_receiver {
            return Err(HotRelayerError::UnexpectedReceiver {
                direction: "withdrawal",
                expected: self.stellar_receiver.clone(),
                actual: pending.withdraw_data.receiver_id.clone(),
            });
        }
        validate_stellar_account(
            "withdrawal",
            "receiver_id",
            &pending.withdraw_data.receiver_id,
        )?;
        Ok(())
    }
}

fn validate_chain_id(chain_id: u64) -> Result<(), String> {
    if chain_id == HOT_STELLAR_CHAIN_ID {
        Ok(())
    } else {
        Err(format!(
            "expected Stellar HOT chain id {HOT_STELLAR_CHAIN_ID}, got {chain_id}"
        ))
    }
}

fn validate_chain_id_matches(
    direction: &'static str,
    actual: u64,
    expected: u64,
) -> Result<(), HotRelayerError> {
    if actual == expected {
        Ok(())
    } else {
        Err(HotRelayerError::InvalidField {
            direction,
            field: "chain_id",
            reason: format!("expected {expected}, got {actual}"),
        })
    }
}

fn validate_nonce(direction: &'static str, nonce: &str) -> Result<(), HotRelayerError> {
    if nonce.is_empty() {
        return Err(HotRelayerError::InvalidField {
            direction,
            field: "nonce",
            reason: "cannot be empty".to_string(),
        });
    }
    if nonce.len() > MAX_HOT_NONCE_LEN {
        return Err(HotRelayerError::InvalidField {
            direction,
            field: "nonce",
            reason: format!("too long, max {MAX_HOT_NONCE_LEN}"),
        });
    }
    if !nonce.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HotRelayerError::InvalidField {
            direction,
            field: "nonce",
            reason: "must be decimal digits".to_string(),
        });
    }
    Ok(())
}

fn validate_token_id(token_id: &str, chain_id: u64) -> Result<(), String> {
    if token_id.is_empty() {
        return Err("cannot be empty".to_string());
    }
    if token_id.len() > MAX_HOT_TOKEN_ID_LEN {
        return Err(format!("too long, max {MAX_HOT_TOKEN_ID_LEN}"));
    }
    let expected_prefix = format!("{chain_id}_");
    if !token_id.starts_with(&expected_prefix) || token_id.len() == expected_prefix.len() {
        return Err(format!(
            "must start with {expected_prefix} and include an asset id"
        ));
    }
    Ok(())
}

fn validate_token_id_matches(
    direction: &'static str,
    actual: &str,
    expected: &str,
    chain_id: u64,
) -> Result<(), HotRelayerError> {
    validate_token_id(actual, chain_id).map_err(|reason| HotRelayerError::InvalidField {
        direction,
        field: "token_id",
        reason,
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(HotRelayerError::InvalidField {
            direction,
            field: "token_id",
            reason: format!("expected {expected}, got {actual}"),
        })
    }
}

fn validate_amount(direction: &'static str, amount: &str) -> Result<(), HotRelayerError> {
    if amount.is_empty() {
        return Err(HotRelayerError::InvalidField {
            direction,
            field: "amount",
            reason: "cannot be empty".to_string(),
        });
    }
    if amount.len() > MAX_HOT_AMOUNT_LEN {
        return Err(HotRelayerError::InvalidField {
            direction,
            field: "amount",
            reason: format!("too long, max {MAX_HOT_AMOUNT_LEN}"),
        });
    }
    if !amount.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HotRelayerError::InvalidField {
            direction,
            field: "amount",
            reason: "must be decimal digits".to_string(),
        });
    }
    let parsed = amount
        .parse::<u128>()
        .map_err(|e| HotRelayerError::InvalidField {
            direction,
            field: "amount",
            reason: e.to_string(),
        })?;
    if parsed == 0 {
        return Err(HotRelayerError::InvalidField {
            direction,
            field: "amount",
            reason: "must be > 0".to_string(),
        });
    }
    Ok(())
}

fn validate_near_receiver(receiver: &str) -> Result<(), String> {
    receiver
        .parse::<AccountId>()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn validate_near_account(
    direction: &'static str,
    field: &'static str,
    account_id: &str,
) -> Result<(), HotRelayerError> {
    validate_near_receiver(account_id).map_err(|reason| HotRelayerError::InvalidField {
        direction,
        field,
        reason,
    })
}

fn validate_stellar_receiver(receiver: &str) -> Result<(), String> {
    ScAddress::from_str(receiver)
        .map(|_| ())
        .map_err(|_| "must be a valid Stellar account or contract address".to_string())
}

fn validate_stellar_account(
    direction: &'static str,
    field: &'static str,
    account_id: &str,
) -> Result<(), HotRelayerError> {
    validate_stellar_receiver(account_id).map_err(|reason| HotRelayerError::InvalidField {
        direction,
        field,
        reason,
    })
}

impl HotMpcApiClient {
    pub fn new(base_url: HotMpcApiUrl, timeout: Duration) -> Result<Self, HotRelayerError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| HotRelayerError::Http(error.to_string()))?;

        Ok(Self { client, base_url })
    }

    #[must_use]
    pub fn from_client(client: reqwest::Client, base_url: HotMpcApiUrl) -> Self {
        Self { client, base_url }
    }

    fn url(&self, path: &str) -> reqwest::Url {
        self.base_url.join(path)
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

    const STELLAR_ACCOUNT: &str = "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV";

    fn routing() -> HotRelayerRouting {
        HotRelayerRouting::new(
            "vault-counterparty.near".to_string(),
            STELLAR_ACCOUNT.to_string(),
            HOT_STELLAR_CHAIN_ID,
            "1100_CUSDC".to_string(),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn valid_deposit_event() -> StellarDepositEvent {
        StellarDepositEvent {
            chain_id: 1100,
            nonce: "21".to_string(),
            sender_id: STELLAR_ACCOUNT.to_string(),
            receiver_id: "vault-counterparty.near".to_string(),
            token_id: "1100_CUSDC".to_string(),
            amount: "42".to_string(),
        }
    }

    fn valid_pending_withdrawal() -> PendingWithdrawal {
        PendingWithdrawal {
            nonce: "991".to_string(),
            chain_id: 1100,
            withdraw_data: PendingWithdrawData {
                receiver_id: STELLAR_ACCOUNT.to_string(),
                amount: "1500".to_string(),
                token_id: "1100_CUSDC".to_string(),
            },
        }
    }

    #[tokio::test]
    async fn withdrawal_plan_uses_nonce_for_sign_and_committed_receiver_for_execution() {
        let signer = RecordingSigner::default();
        let pending = valid_pending_withdrawal();
        let routing = routing();

        let execution = plan_stellar_withdraw_execution_checked(&signer, &pending, &routing)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(execution.nonce, "991");
        assert_eq!(execution.signature, "withdraw-signature");
        assert_eq!(execution.receiver, STELLAR_ACCOUNT);
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
        let event = valid_deposit_event();
        let routing = routing();

        let request = deposit_sign_request_from_event_checked(&event, &routing)
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(request.chain, 1100);
        assert_eq!(request.nonce, "21");
        assert_eq!(request.sender_id, STELLAR_ACCOUNT);
        assert_eq!(request.receiver_id, "vault-counterparty.near");
        assert_eq!(request.token_id, "1100_CUSDC");
        assert_eq!(request.amount, "42");
        assert_eq!(request.autopilot, None);
    }

    #[test]
    fn deposit_sign_request_checked_rejects_unexpected_receiver() {
        let mut event = valid_deposit_event();
        event.receiver_id = "wrong.near".to_string();
        let routing = routing();

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
        let mut pending = valid_pending_withdrawal();
        pending.withdraw_data.receiver_id = "wrong-address".to_string();
        let routing = routing();

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

    #[test]
    fn routing_config_rejects_invalid_values() {
        let error = HotRelayerRouting::new(
            "not a near account".to_string(),
            STELLAR_ACCOUNT.to_string(),
            HOT_STELLAR_CHAIN_ID,
            "1100_CUSDC".to_string(),
        )
        .expect_err("expected invalid NEAR receiver");
        assert!(matches!(
            error,
            HotRelayerError::InvalidRouting {
                field: "near_receiver",
                ..
            }
        ));

        let error = HotRelayerRouting::new(
            "vault-counterparty.near".to_string(),
            "not-stellar".to_string(),
            HOT_STELLAR_CHAIN_ID,
            "1100_CUSDC".to_string(),
        )
        .expect_err("expected invalid Stellar receiver");
        assert!(matches!(
            error,
            HotRelayerError::InvalidRouting {
                field: "stellar_receiver",
                ..
            }
        ));

        let error = HotRelayerRouting::new(
            "vault-counterparty.near".to_string(),
            STELLAR_ACCOUNT.to_string(),
            HOT_STELLAR_CHAIN_ID,
            "1101_CUSDC".to_string(),
        )
        .expect_err("expected token/chain mismatch");
        assert!(matches!(
            error,
            HotRelayerError::InvalidRouting {
                field: "token_id",
                ..
            }
        ));
    }

    #[test]
    fn deposit_validation_rejects_chain_token_amount_and_nonce_mismatches() {
        let routing = routing();

        let mut event = valid_deposit_event();
        event.chain_id = 1101;
        assert!(matches!(
            deposit_sign_request_from_event_checked(&event, &routing),
            Err(HotRelayerError::InvalidField {
                field: "chain_id",
                ..
            })
        ));

        let mut event = valid_deposit_event();
        event.token_id = "1100_OTHER".to_string();
        assert!(matches!(
            deposit_sign_request_from_event_checked(&event, &routing),
            Err(HotRelayerError::InvalidField {
                field: "token_id",
                ..
            })
        ));

        let mut event = valid_deposit_event();
        event.amount = "0".to_string();
        assert!(matches!(
            deposit_sign_request_from_event_checked(&event, &routing),
            Err(HotRelayerError::InvalidField {
                field: "amount",
                ..
            })
        ));

        let mut event = valid_deposit_event();
        event.nonce = "nonce-21".to_string();
        assert!(matches!(
            deposit_sign_request_from_event_checked(&event, &routing),
            Err(HotRelayerError::InvalidField { field: "nonce", .. })
        ));
    }

    #[tokio::test]
    async fn withdrawal_validation_rejects_chain_token_amount_and_nonce_mismatches() {
        let signer = RecordingSigner::default();
        let routing = routing();

        let mut pending = valid_pending_withdrawal();
        pending.chain_id = 1101;
        assert!(matches!(
            plan_stellar_withdraw_execution_checked(&signer, &pending, &routing).await,
            Err(HotRelayerError::InvalidField {
                field: "chain_id",
                ..
            })
        ));

        let mut pending = valid_pending_withdrawal();
        pending.withdraw_data.token_id = "1100_OTHER".to_string();
        assert!(matches!(
            plan_stellar_withdraw_execution_checked(&signer, &pending, &routing).await,
            Err(HotRelayerError::InvalidField {
                field: "token_id",
                ..
            })
        ));

        let mut pending = valid_pending_withdrawal();
        pending.withdraw_data.amount = "1.5".to_string();
        assert!(matches!(
            plan_stellar_withdraw_execution_checked(&signer, &pending, &routing).await,
            Err(HotRelayerError::InvalidField {
                field: "amount",
                ..
            })
        ));

        let mut pending = valid_pending_withdrawal();
        pending.nonce = "".to_string();
        assert!(matches!(
            plan_stellar_withdraw_execution_checked(&signer, &pending, &routing).await,
            Err(HotRelayerError::InvalidField { field: "nonce", .. })
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

        let client = HotMpcApiClient::new(
            HotMpcApiUrl::parse(&server.uri()).unwrap_or_else(|e| panic!("{e}")),
            Duration::from_secs(1),
        )
        .unwrap_or_else(|e| panic!("{e}"));
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

        let client = HotMpcApiClient::new(
            HotMpcApiUrl::parse(&server.uri()).unwrap_or_else(|e| panic!("{e}")),
            Duration::from_secs(1),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let signature = client
            .deposit_sign(&request)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(signature, "sig-2");
    }
}
