use std::{
    str::FromStr,
    time::{Duration, SystemTime},
};

use axum::{extract::State, Json};
use near_jsonrpc_client::{
    errors::{JsonRpcError, JsonRpcServerError},
    methods::query::RpcQueryError,
};
use near_primitives::hash::CryptoHash;
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId, NearToken,
};

use templar_universal_account::{
    authentication::{
        passkey::{
            self,
            data::{AuthenticatorData, ClientDataJson},
            Passkey, PasskeySignatureData,
        },
        with_raw_string::WithRawString,
        MessageWithSignature,
    },
    ExecuteArgs, ExecuteArgsMessage, KeyId, PayloadExecutionParameters,
};

use crate::{
    app::App,
    client::near::DeployArgs,
    route::{universal_account::public_key_to_account_id_slug, SimpleResponse},
};

use super::pow::{Pow, PowTarget};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct OldPasskey {
    pub message: passkey::Message<Pow<CreatePasskeyAccount>>,
    pub authenticator_data: AuthenticatorData,
    pub client_data_json: WithRawString<ClientDataJson>,
    pub signature: passkey::signature::Signature,
}

impl OldPasskey {
    pub fn passkey(&self) -> Passkey {
        self.message
            .0
            .parsed
            .payload_ref()
            .payload_unchecked()
            .key
            .clone()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct CreatePasskeyAccount {
    pub key: Passkey,
    pub block_hash: CryptoHash,
}

impl PowTarget for CreatePasskeyAccount {
    fn pow_target(&self) -> String {
        format!("{},{}", &self.key, &self.block_hash)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[serde(crate = "near_sdk::serde")]
pub struct CreateUniversalAccount {
    pub key: KeyId,
    pub block_hash: CryptoHash,
}

impl PowTarget for CreateUniversalAccount {
    fn pow_target(&self) -> String {
        format!("{},{}", &self.key, &self.block_hash)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum CreateRequest {
    Passkey(Box<OldPasskey>),
    #[serde(untagged)]
    ExecuteArgs(ExecuteArgs<Pow<CreateUniversalAccount>>),
}

#[derive(thiserror::Error, Debug, Clone)]
#[error("The key to add did not sign the payload: signer {signer} != to add {to_add}")]
struct KeyIdMismatchError {
    signer: KeyId,
    to_add: KeyId,
}

impl CreateRequest {
    fn key_id_to_add(&self) -> Result<KeyId, Box<KeyIdMismatchError>> {
        match self {
            Self::Passkey(m) => Ok(m.passkey().into()),
            Self::ExecuteArgs(ea) => {
                let signer = ea.key_id();
                let to_add = &ea.message_unchecked().payload_unchecked().key;
                if to_add == &signer {
                    Ok(signer)
                } else {
                    Err(Box::new(KeyIdMismatchError {
                        signer,
                        to_add: to_add.clone(),
                    }))
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct CreateResponse {
    pub account_id: AccountId,
    pub transaction_hash: CryptoHash,
}

#[allow(clippy::too_many_lines)]
#[tracing::instrument(
    name = "create_universal_account",
    skip(app, request),
    fields(key = tracing::field::Empty)
)]
pub async fn create(
    State(app): State<App>,
    Json(request): Json<CreateRequest>,
) -> SimpleResponse<CreateResponse> {
    tracing::info!("Creating universal account");

    let key_id = match request.key_id_to_add() {
        Ok(k) => k,
        Err(e) => {
            tracing::debug!(e = ?e, "Key ID mismatch");
            return SimpleResponse::Rejected {
                reason: e.to_string(),
            };
        }
    };
    tracing::Span::current().record("key_id", tracing::field::display(&key_id));

    let create = match request {
        CreateRequest::Passkey(mws) => {
            let key_inner = mws.passkey();
            let exec_args: ExecuteArgs<_> = ExecuteArgsMessage {
                key: key_inner.clone(),
                mws: Box::new(MessageWithSignature {
                    message: mws.message,
                    signature: mws.signature,
                    auxiliary: PasskeySignatureData {
                        authenticator_data: mws.authenticator_data,
                        client_data_json: mws.client_data_json,
                    },
                }),
            }
            .into();

            let m = match exec_args.verify(
                &PayloadExecutionParameters::builder(app.args.ua.chain_id)
                    .zero()
                    .verifying_contract(app.args.ua.account_id.clone())
                    .build_salt(),
                |o| app.args.ua.is_origin_allowed(o),
            ) {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(error = ?e, "Failed verification");
                    return SimpleResponse::Rejected {
                        reason: format!("Failed verification: {e}"),
                    };
                }
            };

            let p = match m.verify_pow(app.args.ua.pow_difficulty) {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(error = ?e, "Failed proof-of-work");
                    return SimpleResponse::Rejected {
                        reason: e.to_string(),
                    };
                }
            };

            CreateUniversalAccount {
                key: KeyId::Passkey(key_inner),
                block_hash: p.block_hash,
            }
        }
        CreateRequest::ExecuteArgs(request) => {
            let m = match request.verify(
                &PayloadExecutionParameters::builder(app.args.ua.chain_id)
                    .zero()
                    .verifying_contract(app.args.ua.account_id.clone())
                    .build_salt(),
                |o| app.args.ua.is_origin_allowed(o),
            ) {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(error = ?e, "Failed verification");
                    return SimpleResponse::Rejected {
                        reason: format!("Failed verification: {e}"),
                    };
                }
            };

            let p = match m.verify_pow(app.args.ua.pow_difficulty) {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(error = ?e, "Failed proof-of-work");
                    return SimpleResponse::Rejected {
                        reason: e.to_string(),
                    };
                }
            };

            p.clone()
        }
    };

    // Check block timestamp (make sure signature is not too old)

    let block_hash = create.block_hash;
    let Ok(block_timestamp_ms) = app.ua_near.fetch_block_timestamp_ms(block_hash).await else {
        return SimpleResponse::Failure {
            error: "Failed to fetch block timestamp".to_string(),
        };
    };

    let Some(block_timestamp) =
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(block_timestamp_ms))
    else {
        return SimpleResponse::Failure {
            error: "Failed to calculate block age".to_string(),
        };
    };

    if !block_timestamp
        .elapsed()
        .is_ok_and(|duration| duration <= app.args.ua.blockref_max_age)
    {
        tracing::debug!("Rejected: Block reference is too old");
        return SimpleResponse::Rejected {
            reason: "Block reference is too old".to_string(),
        };
    }

    // Check that account does not exist already

    let account_slug = public_key_to_account_id_slug(&create.key);
    tracing::info!("Account slug: {account_slug}");

    let registry_id = &app.args.ua.registry_id;
    let account_id = match AccountId::from_str(&format!("{account_slug}.{registry_id}")) {
        Ok(account_id) => account_id,
        Err(e) => {
            tracing::error!("Failed to construct account ID: {e}");
            return SimpleResponse::Failure {
                error: "Failed to construct account ID".to_string(),
            };
        }
    };

    // Check that account does not exist by fetching the balance and looking
    // for "unknown account" error.
    match app.ua_near.fetch_near_balance(account_id.clone()).await {
        Err(JsonRpcError::ServerError(JsonRpcServerError::HandlerError(
            RpcQueryError::UnknownAccount { .. },
        ))) => { /* Account does not exist already: continue. */ }
        Ok(_) => {
            return SimpleResponse::Rejected {
                reason: "Account already exists".to_string(),
            };
        }
        Err(e) => {
            tracing::error!("Error detecting account existence: {e}");
            return SimpleResponse::Failure {
                error: "Failed to detect whether account exists".to_string(),
            };
        }
    }

    // Create transaction.
    let signed_transaction = app
        .ua_near
        .construct_deploy_from_registry_transaction(
            &app.cache,
            app.args.ua.registry_id.clone(),
            &DeployArgs::new(
                account_slug,
                app.args.ua.version_key.clone(),
                &templar_universal_account::InitArgs {
                    key: create.key,
                    chain_id: app.args.ua.chain_id.into(),
                },
                None,
            ),
        )
        .await;

    // NOTE: This only counts gas from function calls, but this is OK, because
    // the deploy-from-registry transaction is a function call.
    let gas_estimate = near_sdk::Gas::from_gas(
        signed_transaction
            .transaction
            .actions()
            .iter()
            .map(|a| a.get_prepaid_gas().as_gas())
            .sum(),
    );

    let Some(gas_cost_estimate) = app.estimate_cost_of_gas(gas_estimate).await else {
        return SimpleResponse::Failure {
            error: "Gas cost estimation failure".to_string(),
        };
    };

    if let Err(e) = app
        .database
        .create_account(&account_id, app.args.relay.starting_allowance_yocto)
        .await
    {
        tracing::error!("Failed to create account in database: {e}");
        return SimpleResponse::Failure {
            error: "Failed to create account in database".to_string(),
        };
    }

    let transaction_hash = signed_transaction.get_hash();

    let resolve = match app
        .send_and_resolve_transaction(
            account_id.clone(),
            gas_cost_estimate,
            NearToken::ZERO,
            signed_transaction,
            near_primitives::views::TxExecutionStatus::Included,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to send account contract deployment transaction: {e}");
            return SimpleResponse::Failure {
                error: "Failed to send account contract deployment transaction".to_string(),
            };
        }
    };

    // Resolve the transaction in our DB asynchronously.
    tokio::spawn(async move {
        if let Err(e) = resolve.await {
            tracing::error!("Failed to resolve transaction: {e}");
        }
    });

    SimpleResponse::success(CreateResponse {
        account_id,
        transaction_hash,
    })
}

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;
    use p256::elliptic_curve::rand_core::OsRng;
    use solana_sdk::{signature::Keypair, signer::Signer};
    use templar_universal_account::{
        authentication::{ed25519::raw, HashForSigning, Payload},
        NEAR_TESTNET_CHAIN_ID,
    };

    use super::*;

    #[test]
    fn encoding_ed25519_raw() {
        let keypair = Keypair::new();
        let pubkey = raw::VerifyKey(keypair.pubkey().to_bytes().into());

        let message = {
            let m = raw::Message::from_parsed(Payload::new(
                PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                    .zero()
                    .verifying_contract(AccountId::from_str("my-universal-account.near").unwrap())
                    .build_salt(),
                Pow::mine(
                    CreateUniversalAccount {
                        key: pubkey.clone().into(),
                        block_hash: CryptoHash([0u8; 32]),
                    },
                    2,
                    10_000,
                )
                .unwrap(),
            ));
            let h = m.preimage_for_signing();
            let signature = *keypair.sign_message(&h).as_array();
            Box::new(m.with_signature(signature.into()))
        };

        let cr = CreateRequest::ExecuteArgs(
            ExecuteArgsMessage {
                key: pubkey.clone(),
                mws: message.clone(),
            }
            .into(),
        );

        eprintln!("{cr:?}");
        eprintln!("{}", near_sdk::serde_json::to_string_pretty(&cr).unwrap());

        let parsed: CreateRequest =
            serde_json::from_str(&serde_json::to_string(&cr).unwrap()).unwrap();

        assert_eq!(parsed.key_id_to_add().unwrap(), cr.key_id_to_add().unwrap());

        let original_message = message;

        let CreateRequest::ExecuteArgs(ExecuteArgs::Ed25519Raw(ExecuteArgsMessage {
            key,
            mws: message,
        })) = parsed
        else {
            panic!("invalid parse");
        };

        assert_eq!(key, pubkey);
        assert_eq!(message, original_message);
    }

    #[test]
    fn encoding_passkey() {
        let keypair = p256::SecretKey::random(&mut OsRng);
        let pubkey = Passkey(keypair.public_key().into());

        let cr = CreateRequest::ExecuteArgs(
            ExecuteArgsMessage {
                key: pubkey.clone(),
                mws: {
                    let m = passkey::Message::from_parsed(Payload::new(
                        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                            .zero()
                            .verifying_contract(
                                AccountId::from_str("my-universal-account.near").unwrap(),
                            )
                            .build_salt(),
                        Pow::mine(
                            CreateUniversalAccount {
                                key: pubkey.into(),
                                block_hash: CryptoHash([0u8; 32]),
                            },
                            2,
                            10_000,
                        )
                        .unwrap(),
                    ));
                    let challenge = m.hash_for_signing().into();
                    Box::new(m.sign(
                        &keypair,
                        passkey::data::AuthenticatorData([1u8; 32].into()),
                        passkey::data::ClientDataJson {
                            r#type: "type".to_string(),
                            challenge,
                            origin: "origin".to_string(),
                            cross_origin: None,
                            top_origin: None,
                        },
                    ))
                },
            }
            .into(),
        );

        eprintln!("{cr:?}");
        eprintln!("{}", near_sdk::serde_json::to_string_pretty(&cr).unwrap());

        let inverse: CreateRequest =
            near_sdk::serde_json::from_str(&near_sdk::serde_json::to_string(&cr).unwrap()).unwrap();

        assert!(
            matches!(
                inverse,
                CreateRequest::ExecuteArgs(ExecuteArgs::Passkey { .. }),
            ),
            "should parse to new format"
        );
    }

    #[test]
    fn can_still_parse_old_passkey_format() {
        let old = r#"{
              "Passkey": {
                "authenticator_data": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "message": "{\"parameters\":{\"block_height\":\"0\",\"index\":\"0\",\"nonce\":\"0\"},\"account_id\":\"ua_deployer7175948-34077552565862.test.near\",\"payload\":{\"pow_nonce\":\"83\",\"key\":\"p256:NWo1ZFBV1ZWhyfVkHRmZwbBC5pYmUEpEZ1SqtCzA6CrSnHszYwcS5MnaGZWTkqf1scFzhikZvbZhkxxFEjwDC4sd\",\"block_hash\":\"9fu4SxXiTpHH2VsmB2ZKejjtkyrUMY8BMjpS18JsCEfv\"}}",
                "client_data_json": "{\"type\":\"type\",\"challenge\":\"F8e2D6LXKwKC-ua1MjvU_9w814_paOUMbsL7Le9D7Ng\",\"origin\":\"origin\",\"crossOrigin\":null,\"topOrigin\":null}",
                "signature": "MEUCIEmV-RNxjVd7c0kcG-xpIJV7euA5H5sagy3FEcUdxr_8AiEAiHPX_w-DtL-wtHfKnRdW1_JcuyVLK-6ZDliOdtRHWy4"
              }
            }"#;

        let _: CreateRequest = serde_json::from_str(old).unwrap();
    }

    #[test]
    fn parse_passkey_format() {
        let s = r#"{
          "Passkey": {
            "key": "p256:P4c7jb4V1teuCyv73qfVgN1WcaJVAeCzQnkoxu5YHq1bwuPA5tuUCGRwja7hzsAUum5wtGHHK4XBC6PXHKr6naYh",
            "message": {
              "authenticator_data": "0101010101010101010101010101010101010101010101010101010101010101",
              "message": "{\"parameters\":{\"block_height\":\"0\",\"index\":\"0\",\"nonce\":\"0\"},\"account_id\":\"my-universal-account.near\",\"payload\":{\"pow_nonce\":\"1\",\"key\":{\"Passkey\":\"p256:P4c7jb4V1teuCyv73qfVgN1WcaJVAeCzQnkoxu5YHq1bwuPA5tuUCGRwja7hzsAUum5wtGHHK4XBC6PXHKr6naYh\"},\"block_hash\":\"11111111111111111111111111111111\"}}",
              "client_data_json": "{\"type\":\"type\",\"challenge\":\"3At7GUHxL-iY9xFau_22Gj3wdrwF-CposghU3ymMk-Q\",\"origin\":\"origin\",\"crossOrigin\":null,\"topOrigin\":null}",
              "signature": "MEYCIQCeoz4IlQC0AmVqdUqr8KWhGw83RZ7tPAojhYbtNnpS3AIhAICzyzaaE_ZK-KUihjuHdYplsLSvVWHGi8fyYGu4I4m3"
            }
          }
        }"#;

        let _: CreateRequest = serde_json::from_str(s).unwrap();
    }

    #[test]
    fn parse_ed25519_raw_format() {
        let s = r#"{
          "Ed25519Raw": {
            "key": "ed25519:73B9bxzgHd7xskqX4Q2qGUZgY563JdrFwit12AYr8A2M",
            "message": {
              "message": "{\"parameters\":{\"block_height\":\"0\",\"index\":\"0\",\"nonce\":\"0\"},\"account_id\":\"my-universal-account.near\",\"payload\":{\"pow_nonce\":\"4\",\"key\":{\"Ed25519RawKey\":\"ed25519:73B9bxzgHd7xskqX4Q2qGUZgY563JdrFwit12AYr8A2M\"},\"block_hash\":\"11111111111111111111111111111111\"}}",
              "signature": "ed25519:2S44PvfeEq68pJzQYNtUbnwfo5rWKr37ZBpKGijJaAm8AdrhXLQLEprw3e5DUCeK7tHRKFUrfXfeYcWU3ciysf5R"
            }
          }
        }"#;

        let _: CreateRequest = serde_json::from_str(s).unwrap();
    }
}
