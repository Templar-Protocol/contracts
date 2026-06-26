use std::{collections::HashSet, fmt::Write};

use axum::{extract::State, Json};
use near_primitives::hash::CryptoHash;
use near_sdk::{
    serde::{Deserialize, Serialize},
    serde_json, AccountId, Gas, NearToken,
};
use templar_gateway_core::GatewayError;
use templar_gateway_methods_spec::{contract, universal_account as ua};
use templar_gateway_types::common::ContractArgs;
use templar_universal_account::{
    transaction::{Action, Transaction},
    ExecuteArgs, KeyId, KeyParameters, PayloadExecutionParameters,
};

use crate::{app::App, route::SimpleResponse};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct RelayRequest {
    pub account_id: AccountId,
    pub args: serde_json::Value,
    #[serde(default)]
    pub storage_deposit: HashSet<AccountId>,
    #[serde(default)]
    pub update_prices: bool,
}

impl RelayRequest {
    /// # Errors
    ///
    /// - Serialization of arguments
    pub fn new(
        account_id: AccountId,
        args: impl Into<ExecuteArgs<Box<[Transaction]>>>,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            account_id,
            args: serde_json::to_value(args.into())?,
            storage_deposit: HashSet::default(),
            update_prices: false,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct RelayResponse {
    pub transaction_hash: CryptoHash,
}

#[allow(clippy::too_many_lines)]
#[tracing::instrument(name = "relay_universal_account", skip(app))]
pub async fn relay(
    State(app): State<App>,
    Json(RelayRequest {
        account_id,
        args: args_raw,
        storage_deposit,
        update_prices,
    }): Json<RelayRequest>,
) -> SimpleResponse<RelayResponse> {
    tracing::info!("Processing universal account relay");

    // This is a stopgap measure to support the old args passed by the FE.
    // Once the FE is fully-upgraded to support the new args format, this
    // should be removed, and we should deserialize `args` to `ExecuteArgs`
    // directly in `RelayRequest`.
    let args = match serde_json::to_string(&args_raw)
        .and_then(|s| serde_json::from_str::<ExecuteArgs<Box<[Transaction]>>>(&s))
    {
        Ok(a) => a,
        Err(e) => {
            let msg = format!("Invalid args: {e}");
            tracing::info!("{msg}");
            return SimpleResponse::Rejected { reason: msg };
        }
    };

    let parameters = match load_ua_key(&app, account_id.clone(), args.key_id()).await {
        Ok(parameters) => parameters,
        Err(e) => {
            // Account might not exist, but we also might have connection issues.
            tracing::warn!("Failed to load execution parameters for key \"{}\" from universal account \"{account_id}\": {e}", args.key_id());
            return SimpleResponse::Failure {
                error: "Failed to load execution parameters from universal account".to_string(),
            };
        }
    };

    let Some(parameters) = parameters else {
        tracing::info!(
            "Key \"{}\" does not exist on account \"{account_id}\"",
            args.key_id(),
        );
        return SimpleResponse::Rejected {
            reason: "Key does not exist on account".to_string(),
        };
    };

    let payload = match args.clone().verify(&parameters.next_nonce(), |o| {
        app.args.ua.is_origin_allowed(o)
    }) {
        Ok(p) => p,
        Err(e) => {
            tracing::info!("Verification failed: {e}");
            return SimpleResponse::Rejected {
                reason: format!("Verification failed: {e}"),
            };
        }
    };

    let accounts = app.accounts.read().await;

    let mut gas = near_sdk::Gas::from_tgas(app.args.ua.execute_tgas).as_gas();
    let mut interacted_contract_ids = HashSet::with_capacity(payload.len());
    for transaction in payload {
        let receiver_id = &transaction.receiver_id;
        if receiver_id == &account_id {
            // Reflexive action - allow all.
            // One exception: recursive "execute" call, since that could be used to bypass gas restrictions.
            // There is not a good use-case for this anyways, so it should be okay to reject wholesale.
            for a in &transaction.actions {
                match a {
                    Action::FunctionCall(call) | Action::FunctionCallWeight { call, .. }
                        if call.function_name == "execute" =>
                    {
                        tracing::info!("Rejecting recursive `execute` call.");
                        return SimpleResponse::Rejected {
                            reason: "Recursive `execute` call".to_string(),
                        };
                    }
                    _ => {}
                }
            }

            // Sum gas from any function-call actions for the allowance-lock
            // estimate. The gateway attaches the maximum gas to `ua.execute`
            // and we reconcile against the actual `tokens_burnt`, so this need
            // not account for the (refunded) cost of non-call actions.
            gas += transaction
                .actions
                .iter()
                .filter_map(|a| match a {
                    Action::FunctionCall(call) | Action::FunctionCallWeight { call, .. } => {
                        Some(call.gas.as_gas())
                    }
                    _ => None,
                })
                .sum::<u64>();
            tracing::debug!(transaction = ?transaction, "Transaction is reflexive: allowing.");
            continue;
        }
        let Some(contract_data) = accounts.allowed_contract_data.get(receiver_id) else {
            tracing::info!("Unknown receiver {receiver_id}");
            return SimpleResponse::Rejected {
                reason: "Unknown receiver".to_string(),
            };
        };
        let calls = match transaction
            .actions
            .iter()
            .map(|action| match action {
                Action::FunctionCall(call) | Action::FunctionCallWeight { call, .. } => {
                    Ok((**call).clone().into())
                }
                a => Err(a),
            })
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(calls) => calls,
            Err(a) => {
                tracing::info!("Disallowed action: {a:?}");
                return SimpleResponse::Rejected {
                    reason: "Disallowed action".to_string(),
                };
            }
        };
        let additional_interactions =
            match app.actions_are_allowed(&accounts, receiver_id, contract_data, calls.iter()) {
                Ok(a) => a,
                Err(e) => {
                    tracing::info!("Rejecting payload for reason: {e:?}");
                    let mut s = e[0].to_string();
                    for err in &e[1..] {
                        let _ = write!(&mut s, "\n{err}");
                    }
                    return SimpleResponse::Rejected { reason: s };
                }
            };
        interacted_contract_ids.insert(receiver_id.to_owned());
        interacted_contract_ids.extend(additional_interactions.into_iter());
        gas += calls.iter().map(|f| f.gas.as_gas()).sum::<u64>();
    }

    App::expand_market_related_contracts(&accounts, &mut interacted_contract_ids);
    let market_ids = App::resolve_market_ids(&accounts, &interacted_contract_ids);

    let storage_deposit = interacted_contract_ids.intersection(&storage_deposit);

    // Deposit for storage before sending the user's transaction.
    for contract_id in storage_deposit {
        let Some(contract_data) = accounts.allowed_contract_data.get(contract_id) else {
            continue;
        };

        if let Err(e) = app
            .storage_deposit_top_up(contract_data, contract_id.clone(), account_id.clone())
            .await
        {
            tracing::warn!(error = %e, "Storage deposit error");
            return SimpleResponse::Failure {
                error: format!("Storage deposit error: {e}"),
            };
        }
    }

    drop(accounts);

    if update_prices {
        if let Err(error) = app.update_market_prices(&market_ids).await {
            return SimpleResponse::Failure {
                error: error.to_string(),
            };
        }
    }

    let Some(cost_of_gas) = app.estimate_cost_of_gas(Gas::from_gas(gas)).await else {
        tracing::error!("Failed to estimate cost of gas");
        return SimpleResponse::Failure {
            error: "Failed to estimate cost of gas".to_string(),
        };
    };

    // The relay account signs and pays; the UA account is charged. The gateway
    // attaches the maximum gas to `ua.execute` and surfaces the real cost.
    let transaction_hash = match app
        .execute_and_account(
            account_id.clone(),
            app.args.relay.account_id.clone(),
            cost_of_gas,
            NearToken::from_near(0),
            ua::Execute { account_id, args },
        )
        .await
    {
        Ok(transaction_hash) => transaction_hash,
        Err(e) => {
            tracing::error!("Universal account relay failure: {e}");
            return SimpleResponse::Failure {
                error: format!("Universal account relay failure: {e}"),
            };
        }
    };

    RelayResponse { transaction_hash }.into()
}

/// Load a key's execution parameters from a universal account through the
/// gateway's generic contract view, preserving the relayer's typed handling of
/// the versioned `get_key` response.
pub async fn load_ua_key(
    app: &App,
    ua_account_id: AccountId,
    key: KeyId,
) -> Result<Option<PayloadExecutionParameters>, GatewayError> {
    let result = app
        .gateway
        .read(contract::ViewFunction {
            contract_id: ua_account_id.clone(),
            method_name: "get_key".to_string().into(),
            args: ContractArgs::Json(serde_json::json!({ "key": key })),
        })
        .await?;

    let view: Option<VersionedKeyParameters> = serde_json::from_value(result.value)
        .map_err(|error| GatewayError::NearQuery(format!("invalid get_key response: {error}")))?;

    Ok(view.map(|v| match v {
        VersionedKeyParameters::V1(p) => p,
        VersionedKeyParameters::V0(p) => PayloadExecutionParameters::builder_empty()
            .with_key_parameters(p)
            .verifying_contract(ua_account_id)
            .build(),
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
enum VersionedKeyParameters {
    #[serde(untagged)]
    V1(PayloadExecutionParameters),
    #[serde(untagged)]
    V0(KeyParameters),
}
