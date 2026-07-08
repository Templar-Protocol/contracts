//! Multi-step teardown and asset-recovery flows: removing versions, markets, and
//! registries, and recovering token balances before deleting an account.

use near_account_id::AccountId;
use serde_json::json;
use templar_gateway_client::Client;
use templar_gateway_types::{common::Pagination, ManagedAccountId};

use crate::commands::recover::RecoverNep141;
use crate::commands::registry;
use crate::context::{print_json, CliContext};

/// Recover a NEP-141 balance from the signer to a beneficiary, then unregister
/// the signer's storage — the standalone `recover-nep141` command. Re-reads the
/// balance before unregistering so a failed transfer can't strand tokens.
pub(super) async fn recover_nep141(ctx: CliContext, args: RecoverNep141) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::{storage, token};

    let signer = ctx.signer_account()?;
    let account_id = signer.0.clone();
    let token = token::TokenReference::Ft {
        contract_id: args.token_id.clone(),
    };

    let balance = ctx
        .client
        .read(token::GetBalanceOf {
            token: token.clone(),
            account_id: account_id.clone(),
        })
        .await?
        .balance
        .0;

    if balance > 0 {
        let result = ctx
            .client
            .execute_as(
                signer.clone(),
                token::Transfer {
                    token: token.clone(),
                    receiver_id: args.beneficiary_id.clone(),
                    amount: balance.into(),
                    memo: None,
                },
            )
            .await?;
        ctx.report_tx(&result);
    }

    // Re-read before unregistering: a failed/partial transfer must not lead to
    // unregistering storage while tokens remain (which would strand them).
    let remaining = ctx
        .client
        .read(token::GetBalanceOf { token, account_id })
        .await?
        .balance
        .0;
    if remaining != 0 {
        anyhow::bail!(
            "non-zero balance ({remaining}) remains after transferring to {}; \
             refusing to unregister storage",
            args.beneficiary_id
        );
    }

    let result = ctx
        .client
        .execute_as(
            signer,
            storage::Unregister {
                contract_id: args.token_id,
                force: args.force,
            },
        )
        .await?;
    ctx.report_tx(&result);
    print_json(&result)
}

/// Remove a single registry version, or every version with `--all`.
pub(super) async fn remove_version(
    ctx: CliContext,
    args: registry::RemoveVersion,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::registry as spec;

    // clap's arg group guarantees exactly one of --version-key / --all, so a
    // present single spec is the single-version case; its absence means --all.
    if let Some(spec) = args.single() {
        return ctx.write(spec).await;
    }

    let signer = ctx.signer_account()?;
    let versions = ctx
        .client
        .read(spec::ListVersions {
            registry_id: args.registry_id().clone(),
            args: all_pages(),
        })
        .await?
        .values;

    let mut removed = Vec::new();
    for version_key in versions {
        let result = ctx
            .client
            .execute_as(signer.clone(), args.spec_for(version_key.clone()))
            .await?;
        ctx.report_tx(&result);
        removed.push(version_key);
    }
    print_json(&json!({ "removed": removed }))
}

/// Remove every version from the registry, then delete the (signer) registry
/// account, sweeping its balance to the beneficiary.
pub(super) async fn registry_remove(ctx: CliContext, args: registry::Remove) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::{account, registry as spec};

    let signer = ctx.signer_account()?;
    let registry_id = signer.0.clone();

    let versions = ctx
        .client
        .read(spec::ListVersions {
            registry_id: registry_id.clone(),
            args: all_pages(),
        })
        .await?
        .values;
    for version_key in versions {
        let result = ctx
            .client
            .execute_as(
                signer.clone(),
                spec::RemoveVersion {
                    registry_id: registry_id.clone(),
                    version_key,
                },
            )
            .await?;
        ctx.report_tx(&result);
    }

    let result = ctx
        .client
        .execute_as(
            signer,
            account::Delete {
                beneficiary_id: args.beneficiary_id().clone(),
            },
        )
        .await?;
    ctx.report_tx(&result);
    print_json(&result)
}

/// Remove every market deployed from the registry, signing each removal as the
/// market account with the shared `--secret-key`.
pub(super) async fn clear_deployments(
    ctx: CliContext,
    args: registry::ClearDeployments,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::registry as spec;
    use templar_gateway_types::ContractKind;

    let beneficiary = args.beneficiary_id();
    let force = args.force();
    // Only markets are torn down here (removal reads a market configuration), so
    // filter by kind rather than trying `remove_market` on every deployment.
    let accounts = ctx
        .client
        .read(spec::ListDeploymentsByKind {
            registry_id: args.registry_id().clone(),
            args: all_pages(),
            kind: ContractKind::Market,
        })
        .await?
        .account_ids;

    let mut removed = Vec::new();
    for account in accounts {
        let client = ctx.signing_client_for(account.clone())?;
        match remove_market(&ctx, &client, account.clone().into(), &beneficiary, force).await {
            Ok(()) => removed.push(account),
            Err(error) if force => {
                tracing::warn!(%account, %error, "failed to remove market; continuing (--force)");
            }
            Err(error) => return Err(error.context(format!("remove market {account}"))),
        }
    }
    print_json(&json!({ "removed": removed }))
}

/// Recover a market's assets to the beneficiary, then delete the market account.
/// Reads and writes go through `client`, whose signer must be the market account
/// (its own removal is self-signed). With `force`, tolerates a failed config
/// read or asset recovery and still deletes the account.
pub(super) async fn remove_market(
    ctx: &CliContext,
    client: &Client,
    market: ManagedAccountId,
    beneficiary: &AccountId,
    force: bool,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::{account, market, token};

    match client
        .read(market::GetConfiguration {
            market_id: market.0.clone(),
        })
        .await
    {
        Ok(configuration) => {
            let assets = [
                token::TokenReference::from(&configuration.borrow_asset),
                token::TokenReference::from(&configuration.collateral_asset),
            ];
            // Sweep every asset's balance first, then reclaim storage once per
            // distinct token contract. Two NEP-245 token ids can share a single
            // contract (several Intents market configs do), and a compliant
            // `storage_unregister(force=false)` rejects while any balance remains
            // — so unregistering per-asset would strand the second asset.
            for asset in &assets {
                if let Err(error) =
                    sweep_token(ctx, client, &market, asset.clone(), beneficiary).await
                {
                    if !force {
                        return Err(error);
                    }
                    tracing::warn!(%error, "failed to recover asset; continuing (--force)");
                }
            }

            let mut reclaimed = std::collections::HashSet::new();
            for asset in &assets {
                let contract_id = token_contract_id(asset);
                if !reclaimed.insert(contract_id.clone()) {
                    continue;
                }
                if let Err(error) = reclaim_storage(ctx, client, &market, contract_id.clone()).await
                {
                    if !force {
                        return Err(error);
                    }
                    tracing::warn!(%error, "failed to reclaim storage; continuing (--force)");
                }
            }
        }
        Err(error) => {
            if !force {
                return Err(anyhow::Error::from(error).context("read market configuration"));
            }
            tracing::warn!(%error, "failed to read market configuration; continuing (--force)");
        }
    }

    let result = client
        .execute_as(
            market,
            account::Delete {
                beneficiary_id: beneficiary.clone(),
            },
        )
        .await?;
    ctx.report_tx(&result);
    Ok(())
}

/// The token contract account backing a reference — shared across NEP-245 token
/// ids deployed on the same contract.
fn token_contract_id(token: &templar_gateway_methods_spec::token::TokenReference) -> &AccountId {
    use templar_gateway_methods_spec::token::TokenReference;
    match token {
        TokenReference::Ft { contract_id } | TokenReference::Mt { contract_id, .. } => contract_id,
    }
}

/// Transfer a token's full balance from `from` to `beneficiary` if non-zero,
/// using the standard-agnostic `token.transfer` so NEP-245 assets work too.
async fn sweep_token(
    ctx: &CliContext,
    client: &Client,
    from: &ManagedAccountId,
    token: templar_gateway_methods_spec::token::TokenReference,
    beneficiary: &AccountId,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::token;

    let balance = client
        .read(token::GetBalanceOf {
            token: token.clone(),
            account_id: from.0.clone(),
        })
        .await?
        .balance
        .0;
    if balance > 0 {
        let result = client
            .execute_as(
                from.clone(),
                token::Transfer {
                    token,
                    receiver_id: beneficiary.clone(),
                    amount: balance.into(),
                    memo: None,
                },
            )
            .await?;
        ctx.report_tx(&result);
    }
    Ok(())
}

/// Reclaim `from`'s NEP-145 storage slot on `contract_id`, but only when it's
/// actually reclaimable: probe the registered slot first. A failed read means
/// the contract has no storage management (e.g. some NEP-245 multi-tokens) —
/// skip it. A present, non-zero slot means unregister should work once every
/// balance on the contract has been swept, so a failure there is a real error
/// and propagates. Call after all of the contract's assets have been swept.
async fn reclaim_storage(
    ctx: &CliContext,
    client: &Client,
    from: &ManagedAccountId,
    contract_id: AccountId,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::storage;

    let registered = match client
        .read(storage::GetBalanceOf {
            contract_id: contract_id.clone(),
            account_id: from.0.clone(),
        })
        .await
    {
        Ok(result) => result
            .balance
            .is_some_and(|balance| balance.total.as_yoctonear() > 0),
        Err(error) => {
            // Expected for tokens without NEP-145 storage management, not a fault.
            tracing::info!(%contract_id, %error, "storage_balance_of unavailable; assuming the token does not manage NEP-145 storage");
            false
        }
    };
    if registered {
        let result = client
            .execute_as(
                from.clone(),
                storage::Unregister {
                    contract_id,
                    force: false,
                },
            )
            .await?;
        ctx.report_tx(&result);
    }
    Ok(())
}

/// A pagination request for "every item" — teardown lists the full set.
fn all_pages() -> Pagination {
    Pagination {
        offset: None,
        limit: None,
    }
}
