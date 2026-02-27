use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
};

use axum::{extract::State, Json};
use near_primitives::{hash::CryptoHash, views::TxExecutionStatus};
use near_sdk::{
    serde::{Deserialize, Serialize},
    serde_json, AccountId, NearToken,
};
use templar_common::oracle::{pyth::PriceIdentifier, OraclePriceId};
use templar_universal_account::{
    transaction::{Action, Transaction},
    ExecuteArgs,
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
    pub update_price_feeds: HashSet<PriceIdentifier>,
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
            update_price_feeds: HashSet::default(),
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
        update_price_feeds,
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

    let parameters = match app
        .ua_near
        .load_ua_key(account_id.clone(), args.key_id())
        .await
    {
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

            let protocol_config = app.cache.protocol_configuration().await;

            gas += transaction
                .actions
                .iter()
                .map(|a| a.gas_cost(receiver_id, true, &protocol_config))
                .reduce(|a, b| a.saturating_add(b))
                .unwrap_or(near_sdk::Gas::from_gas(0))
                .as_gas();
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
        if let Some(market_data) = accounts.market_data.get(receiver_id) {
            interacted_contract_ids.insert(market_data.oracle_id.clone());
            interacted_contract_ids.insert(market_data.borrow.asset.contract_id().to_owned());
            interacted_contract_ids.insert(market_data.collateral.asset.contract_id().to_owned());
        }
        gas += calls.iter().map(|f| f.gas).sum::<u64>();
    }

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

    // Send any requested price updates
    let mut interacted_price_identifiers = HashMap::with_capacity(2);
    for contract_id in &interacted_contract_ids {
        if let Some(market_data) = accounts.market_data.get(contract_id) {
            interacted_price_identifiers.insert(
                market_data.collateral.price_id,
                (
                    market_data.collateral.update_oracle_id.clone(),
                    market_data.collateral.update_price_id.clone(),
                ),
            );
            interacted_price_identifiers.insert(
                market_data.borrow.price_id,
                (
                    market_data.borrow.update_oracle_id.clone(),
                    market_data.borrow.update_price_id.clone(),
                ),
            );
        }
    }

    let (pyth_updates, redstone_updates) = interacted_price_identifiers
        .into_iter()
        .filter(|(price_id, _)| update_price_feeds.contains(price_id))
        .map(|(_, (oracle_id, oracle_price_id))| (oracle_id, oracle_price_id))
        .fold(
            (HashMap::new(), HashMap::new()),
            |(mut pyth, mut redstone), (oracle_id, oracle_price_id)| {
                match oracle_price_id {
                    OraclePriceId::Pyth(id) => {
                        pyth.entry(oracle_id)
                            .or_insert_with(HashSet::new)
                            .insert(id);
                    }
                    OraclePriceId::RedStone(id) => {
                        redstone
                            .entry(oracle_id)
                            .or_insert_with(HashSet::new)
                            .insert(id);
                    }
                }
                (pyth, redstone)
            },
        );

    if !pyth_updates.is_empty() {
        let Some(pyth) = app.pyth.as_ref() else {
            tracing::error!("Pyth client not configured");
            return SimpleResponse::Failure {
                error: "Pyth client not configured".to_string(),
            };
        };

        for (oracle_id, feed_ids) in pyth_updates {
            if let Err(e) = pyth
                .update(oracle_id, feed_ids.into_iter().collect::<Vec<_>>().into())
                .await
            {
                tracing::error!("Pyth update failure: {e}");
                return SimpleResponse::Failure {
                    error: format!("Pyth update failure: {e}"),
                };
            }
        }
    }

    if !redstone_updates.is_empty() {
        let Some(redstone) = app.redstone.as_ref() else {
            tracing::error!("Redstone client not configured");
            return SimpleResponse::Failure {
                error: "Redstone client not configured".to_string(),
            };
        };

        for (oracle_id, feed_ids) in redstone_updates {
            if let Err(e) = redstone
                .update(oracle_id, feed_ids.into_iter().collect::<Vec<_>>().into())
                .await
            {
                tracing::error!("Redstone update failure: {e}");
                return SimpleResponse::Failure {
                    error: format!("Redstone update failure: {e}"),
                };
            }
        }
    }

    // Send the user's transaction
    let signed_transaction = app
        .relay_near
        .construct_ua_execute_transaction(&app.cache, account_id.clone(), &args_raw, gas)
        .await;
    let Some(cost_of_gas) = app.estimate_cost_of_gas(near_sdk::Gas::from_gas(gas)).await else {
        tracing::error!("Failed to estimate cost of gas");
        return SimpleResponse::Failure {
            error: "Failed to estimate cost of gas".to_string(),
        };
    };

    let transaction_hash = signed_transaction.get_hash();

    let resolve_transaction = match app
        .send_and_resolve_transaction(
            account_id,
            cost_of_gas,
            NearToken::from_near(0),
            signed_transaction,
            TxExecutionStatus::Final,
        )
        .await
    {
        Ok(future) => future,
        Err(e) => {
            tracing::error!("Send transaction failure: {e}");
            return SimpleResponse::Failure {
                error: format!("Send transaction failure: {e}"),
            };
        }
    };

    // Resolve asynchronously.
    tokio::spawn(async move {
        if let Err(e) = resolve_transaction.await {
            tracing::error!("Resolve transaction failure: {e}");
        }
    });

    RelayResponse { transaction_hash }.into()
}
