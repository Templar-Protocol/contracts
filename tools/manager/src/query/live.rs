//! Batched typed reads with a command-local cache. Errors are data, never zeroes.

use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
};

use anyhow::Context as _;
use futures::{stream, StreamExt};
use near_account_id::AccountId;
use templar_common::{
    market::{usage_ratio, MarketConfiguration},
    snapshot::Snapshot,
};
use templar_gateway_core::{GatewayError, GatewayResult};
use templar_gateway_methods_spec::{
    account, contract, market, owner, proxy_oracle_governance as governance, registry,
};
use templar_gateway_oracle_updates_dispatch::OracleSourceArgs;
use templar_gateway_types::{common::Pagination, contract::ContractKind, MethodSpec, Version};

use super::{
    model::{Field, FieldPath, NodeKind, Relation, Value},
    prices, Failure,
};
use crate::context::CliContext;

type Values = BTreeMap<AccountId, BTreeMap<FieldPath, Option<Value>>>;
type Nodes = BTreeMap<Vec<Relation>, BTreeMap<AccountId, Option<AccountId>>>;

pub(super) async fn discover(
    ctx: &CliContext,
    registry_id: AccountId,
) -> anyhow::Result<Vec<AccountId>> {
    templar_gateway_client::collect_paginated(NonZeroU32::try_from(100)?, |offset, limit| {
        ctx.client.read(registry::ListDeployments {
            registry_id: registry_id.clone(),
            args: Pagination {
                offset: Some(offset),
                limit: Some(limit),
            },
        })
    })
    .await
    .with_context(|| format!("discover deployments from {registry_id}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Read {
    Kind,
    Version,
    Configuration,
    Snapshot,
    Owner,
    GovernedOracle,
}

impl Read {
    fn operation(self) -> &'static str {
        match self {
            Self::Kind => contract::GetKind::RPC_METHOD,
            Self::Version => contract::GetVersion::RPC_METHOD,
            Self::Configuration => market::GetConfiguration::RPC_METHOD,
            Self::Snapshot => market::GetCurrentSnapshot::RPC_METHOD,
            Self::Owner => owner::GetOwner::RPC_METHOD,
            Self::GovernedOracle => governance::GetProxyOracleId::RPC_METHOD,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReadKey(AccountId, Read);

enum ReadValue {
    Deleted,
    Kind(ContractKind),
    Version(Version<()>),
    Configuration(Box<MarketConfiguration>),
    Snapshot(Snapshot),
    Owner(Option<AccountId>),
    GovernedOracle(AccountId),
}

pub(super) struct Loader<'a> {
    ctx: &'a CliContext,
    sources: &'a OracleSourceArgs,
    concurrency: usize,
    cache: BTreeMap<Read, BTreeMap<AccountId, Option<ReadValue>>>,
    price_cache: BTreeMap<AccountId, Option<prices::ValuationPrice>>,
    pub(super) failures: BTreeSet<Failure>,
}

impl<'a> Loader<'a> {
    pub(super) fn new(
        ctx: &'a CliContext,
        sources: &'a OracleSourceArgs,
        concurrency: usize,
    ) -> Self {
        Self {
            ctx,
            sources,
            concurrency,
            cache: BTreeMap::new(),
            price_cache: BTreeMap::new(),
            failures: BTreeSet::new(),
        }
    }

    pub(super) async fn classify(&mut self, ids: Vec<AccountId>, kind: NodeKind) -> Vec<AccountId> {
        let expected = match kind {
            NodeKind::Market => ContractKind::Market,
            NodeKind::Oracle => ContractKind::ProxyOracle,
            NodeKind::Governance => ContractKind::ProxyGovernance,
        };
        self.load(ids.iter().cloned().map(|id| ReadKey(id, Read::Kind)))
            .await;
        for id in &ids {
            if matches!(
                self.get(id, Read::Kind),
                Some(ReadValue::Kind(ContractKind::Unknown))
            ) {
                self.failures.insert(Failure {
                    account_id: id.clone(),
                    operation: Read::Kind.operation().into(),
                    error: "Unknown contract kind for existing account".into(),
                });
            }
        }
        ids.into_iter().filter(|id| matches!(self.get(id, Read::Kind), Some(ReadValue::Kind(actual)) if *actual == expected)).collect()
    }

    async fn load(&mut self, keys: impl IntoIterator<Item = ReadKey>) {
        let pending = keys
            .into_iter()
            .filter(|key| {
                !self
                    .cache
                    .get(&key.1)
                    .is_some_and(|rows| rows.contains_key(&key.0))
            })
            .collect::<BTreeSet<_>>();
        let ctx = self.ctx;
        let mut results = stream::iter(pending)
            .map(|key| async move {
                let result = read(ctx, &key).await.map_err(|error| format!("{error:#}"));
                (key, result)
            })
            .buffer_unordered(self.concurrency);
        while let Some((key, result)) = results.next().await {
            let value = match result {
                Ok(value) => Some(value),
                Err(error) => {
                    self.failures.insert(Failure {
                        account_id: key.0.clone(),
                        operation: key.1.operation().into(),
                        error,
                    });
                    None
                }
            };
            self.cache.entry(key.1).or_default().insert(key.0, value);
        }
    }

    fn get(&self, id: &AccountId, read: Read) -> Option<&ReadValue> {
        self.cache.get(&read)?.get(id)?.as_ref()
    }

    fn configuration(&self, id: &AccountId) -> Option<&MarketConfiguration> {
        match self.get(id, Read::Configuration)? {
            ReadValue::Configuration(configuration) => Some(configuration),
            _ => None,
        }
    }

    async fn resolve(
        &mut self,
        roots: &[AccountId],
        root_kind: NodeKind,
        paths: &[FieldPath],
    ) -> Nodes {
        let mut nodes: Nodes = BTreeMap::from([(
            Vec::new(),
            roots
                .iter()
                .map(|id| (id.clone(), Some(id.clone())))
                .collect(),
        )]);
        let prefixes = paths
            .iter()
            .flat_map(|path| {
                (1..=path.relations.len()).map(|depth| path.relations[..depth].to_vec())
            })
            .collect::<BTreeSet<_>>();
        for prefix in prefixes {
            let parent = &prefix[..prefix.len() - 1];
            let Some(source_kind) = parent
                .iter()
                .try_fold(root_kind, |kind, relation| relation.target(kind))
            else {
                continue;
            };
            let relation = prefix[prefix.len() - 1];
            let source_ids = roots
                .iter()
                .filter_map(|root| nodes.get(parent)?.get(root)?.clone())
                .collect::<BTreeSet<_>>();
            let required = match (source_kind, relation) {
                (NodeKind::Market, Relation::Oracle) => Read::Configuration,
                (NodeKind::Governance, Relation::Oracle) => Read::GovernedOracle,
                (_, Relation::Governance) => Read::Owner,
                _ => continue,
            };
            if relation == Relation::Governance {
                self.load_governance_relation(&source_ids).await;
            } else {
                self.load(source_ids.iter().cloned().map(|id| ReadKey(id, required)))
                    .await;
            }
            let targets = roots
                .iter()
                .map(|root| {
                    let target = nodes
                        .get(parent)
                        .and_then(|rows| rows.get(root))
                        .and_then(Option::as_ref)
                        .and_then(|source| self.related_id(source, required));
                    (root.clone(), target)
                })
                .collect();
            nodes.insert(prefix, targets);
        }
        nodes
    }

    async fn load_governance_relation(&mut self, sources: &BTreeSet<AccountId>) {
        self.load(sources.iter().cloned().map(|id| ReadKey(id, Read::Kind)))
            .await;
        let proxies = sources
            .iter()
            .filter(|id| {
                matches!(
                    self.get(id, Read::Kind),
                    Some(ReadValue::Kind(ContractKind::ProxyOracle))
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        self.load(proxies.iter().cloned().map(|id| ReadKey(id, Read::Owner)))
            .await;
        let owners = proxies
            .iter()
            .filter_map(|id| match self.get(id, Read::Owner)? {
                ReadValue::Owner(Some(owner)) if owner != id => Some(owner.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        self.load(owners.iter().cloned().map(|id| ReadKey(id, Read::Kind)))
            .await;
        let governance_ids = owners
            .into_iter()
            .filter(|id| {
                matches!(
                    self.get(id, Read::Kind),
                    Some(ReadValue::Kind(ContractKind::ProxyGovernance))
                )
            })
            .collect::<Vec<_>>();
        self.load(
            governance_ids
                .into_iter()
                .map(|id| ReadKey(id, Read::GovernedOracle)),
        )
        .await;
    }

    fn related_id(&self, source: &AccountId, required: Read) -> Option<AccountId> {
        match self.get(source, required)? {
            ReadValue::Configuration(configuration) => {
                Some(configuration.price_oracle_configuration.account_id.clone())
            }
            ReadValue::GovernedOracle(id) => Some(id.clone()),
            ReadValue::Owner(Some(owner)) => match self.get(owner, Read::GovernedOracle)? {
                ReadValue::GovernedOracle(target) if target == source => Some(owner.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) async fn targets(
        &mut self,
        roots: &[AccountId],
        root_kind: NodeKind,
        path: &FieldPath,
    ) -> anyhow::Result<BTreeMap<AccountId, Option<AccountId>>> {
        let mut nodes = self
            .resolve(roots, root_kind, std::slice::from_ref(path))
            .await;
        nodes
            .remove(&path.relations)
            .context("validated relationship prefix was not resolved")
    }

    pub(super) async fn values(
        &mut self,
        roots: &[AccountId],
        root_kind: NodeKind,
        paths: &[FieldPath],
    ) -> Values {
        let nodes = self.resolve(roots, root_kind, paths).await;
        let keys = roots.iter().flat_map(|root| {
            paths
                .iter()
                .filter_map(|path| {
                    let id = nodes.get(&path.relations)?.get(root)?.as_ref()?;
                    Some(
                        dependencies(path.field)
                            .iter()
                            .map(move |read| ReadKey(id.clone(), *read)),
                    )
                })
                .flatten()
        });
        self.load(keys).await;
        if paths.iter().any(|path| path.field.is_usd()) {
            let (ids, configurations): (Vec<_>, Vec<_>) = roots
                .iter()
                .filter(|id| !self.price_cache.contains_key(*id))
                .filter(|id| self.get(id, Read::Snapshot).is_some())
                .filter_map(|id| {
                    Some((
                        id.clone(),
                        self.configuration(id)?.price_oracle_configuration.clone(),
                    ))
                })
                .unzip();
            let prices = prices::fresh_prices(self.ctx, self.sources, &configurations).await;
            for (id, price) in ids.into_iter().zip(prices) {
                let value = match price {
                    Ok(value) => Some(value),
                    Err(error) => {
                        self.failures.insert(Failure {
                            account_id: id.clone(),
                            operation: "oracle.resolveFreshPrice".into(),
                            error,
                        });
                        None
                    }
                };
                self.price_cache.insert(id, value);
            }
        }
        let mut result = BTreeMap::new();
        for root in roots {
            let mut fields = BTreeMap::new();
            for path in paths {
                let target = nodes
                    .get(&path.relations)
                    .and_then(|rows| rows.get(root))
                    .and_then(Option::as_ref);
                let value = match target
                    .map(|id| self.field_value(id, path.field))
                    .transpose()
                {
                    Ok(value) => value.flatten(),
                    Err(error) => {
                        self.failures.insert(Failure {
                            account_id: root.clone(),
                            operation: path.to_string(),
                            error: format!("{error:#}"),
                        });
                        None
                    }
                };
                fields.insert(path.clone(), value);
            }
            result.insert(root.clone(), fields);
        }
        result
    }

    fn field_value(&self, id: &AccountId, field: Field) -> anyhow::Result<Option<Value>> {
        let value = match field {
            Field::Id => Some(Value::Text(id.to_string())),
            Field::Kind => match self.get(id, Read::Kind) {
                Some(ReadValue::Kind(kind)) => Some(Value::Text(super::model::kind_name(*kind)?)),
                _ => None,
            },
            Field::Version => match self.get(id, Read::Version) {
                Some(ReadValue::Version(version)) => Some(Value::Version(*version)),
                _ => None,
            },
            Field::BorrowAsset => self
                .configuration(id)
                .map(|configuration| Value::Asset(configuration.borrow_asset.to_string())),
            Field::CollateralAsset => self
                .configuration(id)
                .map(|configuration| Value::Asset(configuration.collateral_asset.to_string())),
            Field::BorrowApr
            | Field::UsageRatio
            | Field::Borrowed
            | Field::Supplied
            | Field::BorrowedUsd
            | Field::SuppliedUsd => {
                let Some(ReadValue::Snapshot(snapshot)) = self.get(id, Read::Snapshot) else {
                    return Ok(None);
                };
                let value = match field {
                    Field::BorrowApr => snapshot.interest_rate,
                    Field::UsageRatio => usage_ratio(
                        snapshot.borrow_asset_deposited_active,
                        snapshot.borrow_asset_borrowed,
                    ),
                    Field::Borrowed | Field::Supplied | Field::BorrowedUsd | Field::SuppliedUsd => {
                        let Some(configuration) = self.configuration(id) else {
                            return Ok(None);
                        };
                        let amount = match field {
                            Field::Borrowed | Field::BorrowedUsd => snapshot.borrow_asset_borrowed,
                            _ => snapshot.borrow_asset_deposited_active,
                        };
                        let decimals = configuration
                            .price_oracle_configuration
                            .borrow_asset_decimals;
                        if field.is_usd() {
                            let Some(Some(price)) = self.price_cache.get(id) else {
                                return Ok(None);
                            };
                            prices::principal_value(amount.into(), decimals, price)?
                        } else {
                            prices::token_amount(amount.into(), decimals)?
                        }
                    }
                    Field::Id
                    | Field::Kind
                    | Field::Version
                    | Field::BorrowAsset
                    | Field::CollateralAsset => return Ok(None),
                };
                Some(Value::Number(value))
            }
        };
        Ok(value)
    }
}

fn dependencies(field: Field) -> &'static [Read] {
    match field {
        Field::Id => &[],
        Field::Kind => &[Read::Kind],
        Field::Version => &[Read::Version],
        Field::BorrowAsset | Field::CollateralAsset => &[Read::Configuration],
        Field::BorrowApr | Field::UsageRatio => &[Read::Snapshot],
        Field::Borrowed | Field::Supplied | Field::BorrowedUsd | Field::SuppliedUsd => {
            &[Read::Configuration, Read::Snapshot]
        }
    }
}

async fn read(ctx: &CliContext, key: &ReadKey) -> anyhow::Result<ReadValue> {
    let id = &key.0;
    Ok(match key.1 {
        Read::Kind => {
            let kind = ctx
                .client
                .read(contract::GetKind {
                    contract_id: id.clone(),
                })
                .await?;
            if kind == ContractKind::Unknown {
                // GetKind also returns Unknown for deleted accounts. Confirm absence
                // without hiding transport failures or live, unsupported contracts.
                unknown_kind(
                    ctx.client
                        .read(account::Get {
                            account_id: id.clone(),
                        })
                        .await,
                )?
            } else {
                ReadValue::Kind(kind)
            }
        }
        Read::Version => {
            let version = ctx
                .client
                .read(contract::GetVersion {
                    contract_id: id.clone(),
                })
                .await?;
            ReadValue::Version(version.parsed.with_context(|| {
                format!(
                    "live version {:?} is not major.minor.patch",
                    version.version_string
                )
            })?)
        }
        Read::Configuration => ReadValue::Configuration(Box::new(
            ctx.client
                .read(market::GetConfiguration {
                    market_id: id.clone(),
                })
                .await?,
        )),
        Read::Snapshot => ReadValue::Snapshot(
            ctx.client
                .read(market::GetCurrentSnapshot {
                    market_id: id.clone(),
                })
                .await?,
        ),
        Read::Owner => ReadValue::Owner(
            ctx.client
                .read(owner::GetOwner {
                    contract_id: id.clone(),
                })
                .await?,
        ),
        Read::GovernedOracle => ReadValue::GovernedOracle(
            ctx.client
                .read(governance::GetProxyOracleId {
                    governance_id: id.clone(),
                })
                .await?,
        ),
    })
}

fn unknown_kind(result: GatewayResult<account::GetResult>) -> GatewayResult<ReadValue> {
    match result {
        Ok(_) => Ok(ReadValue::Kind(ContractKind::Unknown)),
        Err(GatewayError::AccountNotFound(_)) => Ok(ReadValue::Deleted),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_accounts_are_skipped() {
        assert!(matches!(
            unknown_kind(Err(GatewayError::AccountNotFound(
                "deleted.near".parse().unwrap()
            ))),
            Ok(ReadValue::Deleted)
        ));
    }

    #[test]
    fn existing_codeless_accounts_remain_unknown() {
        let account = account::GetResult {
            amount: templar_gateway_types::NearToken::from_yoctonear(0),
            locked: templar_gateway_types::NearToken::from_yoctonear(0),
            code_hash: "11111111111111111111111111111111".into(),
            storage_usage: 0,
            global_contract_hash: None,
            global_contract_account_id: None,
        };
        assert!(matches!(
            unknown_kind(Ok(account)),
            Ok(ReadValue::Kind(ContractKind::Unknown))
        ));
    }

    #[test]
    fn failed_existence_checks_are_not_deleted_accounts() {
        assert!(matches!(
            unknown_kind(Err(GatewayError::NearQuery(
                "TransportError(connection timed out)".into()
            ))),
            Err(GatewayError::NearQuery(_))
        ));
    }

    #[rstest::rstest]
    #[case(0, 0, "0")]
    #[case(0, 10, "0")]
    #[case(10, 0, "0")]
    #[case(10, 5, "0.5")]
    #[case(10, 10, "1")]
    #[case(10, 11, "1")]
    fn utilization_matches_market_boundaries(
        #[case] active: u128,
        #[case] borrowed: u128,
        #[case] expected: &str,
    ) {
        assert_eq!(
            usage_ratio(active.into(), borrowed.into()),
            expected.parse::<templar_common::Decimal>().unwrap()
        );
    }
}
