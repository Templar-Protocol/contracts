//! Typed, read-only fleet queries. Discovery, loading, and evaluation are separate.

pub(crate) mod assets;
mod live;
pub(crate) mod model;
mod output;
mod prices;

use std::collections::BTreeSet;

use anyhow::Context as _;
use clap::{Args, Subcommand};
use near_account_id::AccountId;
use serde::Serialize;
use templar_gateway_oracle_updates_dispatch::OracleSourceArgs;

use crate::context::{print_json, CliContext};
use model::{compare_values, FieldPath, Predicate, RootKind};

#[derive(Debug, Subcommand)]
pub(crate) enum QueryNs {
    /// Query deployed markets.
    Markets(QueryArgs),
    /// Query deployed proxy oracles.
    ProxyOracles(QueryArgs),
    /// Query deployed proxy-oracle governance contracts.
    ProxyGovernance(QueryArgs),
    /// Discover field names, types, units, operators, and relationships.
    Fields {
        #[arg(value_enum)]
        kind: RootKind,
    },
    /// List built-in asset aliases and their exact wrapped identities.
    Assets,
}

#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("source").args(["registry_id", "account_id"]).required(true)))]
pub(crate) struct QueryArgs {
    /// Discover all deployments of this kind from a registry.
    #[arg(long)]
    registry_id: Option<AccountId>,
    /// Query specific accounts instead of discovering them; repeatable.
    #[arg(long)]
    account_id: Vec<AccountId>,
    /// Typed predicate: 'borrow_apr >= 10%'. Repeated predicates mean AND.
    #[arg(long, value_name = "FIELD OP VALUE")]
    filter: Vec<String>,
    /// Restrict output fields; defaults to all non-USD root fields.
    #[arg(long, value_delimiter = ',', value_name = "FIELD,...")]
    select: Vec<String>,
    /// Include defaults along a relationship path, e.g. oracle.governance; repeatable.
    #[arg(long, value_delimiter = ',', value_name = "RELATION.PATH")]
    include: Vec<String>,
    /// Include fresh indicative USD principal values (markets only).
    #[arg(long)]
    usd: bool,
    /// Sort root rows by a typed field before mapping; missing values sort last.
    #[arg(long, value_name = "FIELD")]
    sort: Option<String>,
    #[arg(long, requires = "sort")]
    descending: bool,
    /// Map matching roots to distinct related contracts, e.g. oracle.governance.
    #[arg(long, value_name = "RELATION.PATH")]
    map: Option<String>,
    /// Limit output after filtering, sorting, and mapping; discovery is not truncated.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    limit: Option<u32>,
    /// Maximum concurrent ordinary contract reads; excludes USD pricing work.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=32))]
    concurrency: u32,
    #[command(flatten)]
    sources: OracleSourceArgs,
}

#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Failure {
    account_id: AccountId,
    operation: String,
    error: String,
}

#[derive(Serialize)]
struct QueryResult {
    scanned: usize,
    matched: usize,
    complete: bool,
    rows: Vec<output::Row>,
    failures: BTreeSet<Failure>,
}

pub(crate) async fn run(ctx: CliContext, command: QueryNs) -> anyhow::Result<()> {
    let (kind, args) = match command {
        QueryNs::Fields { kind } => return print_json(&model::catalog(kind.node_kind())),
        QueryNs::Assets => return print_json(&assets::catalog()),
        QueryNs::Markets(args) => (RootKind::Markets, args),
        QueryNs::ProxyOracles(args) => (RootKind::ProxyOracles, args),
        QueryNs::ProxyGovernance(args) => (RootKind::ProxyGovernance, args),
    };
    execute(&ctx, kind, args).await
}

async fn execute(ctx: &CliContext, kind: RootKind, args: QueryArgs) -> anyhow::Result<()> {
    let root = kind.node_kind();
    let mut predicates = args
        .filter
        .iter()
        .map(|text| Predicate::parse(root, text))
        .collect::<anyhow::Result<Vec<_>>>()?;
    // Cheap predicates can discard rows before any provider is contacted.
    predicates.sort_by_key(|predicate| predicate.path.field.is_usd());
    let mapping = args
        .map
        .as_ref()
        .map(|path| FieldPath::parse(root, &format!("{path}.id")))
        .transpose()?;
    let output_kind = mapping.as_ref().map_or(root, FieldPath::endpoint_kind);
    let selected = output::fields(output_kind, &args.select, &args.include, args.usd)?;
    let sort = args
        .sort
        .as_deref()
        .map(|text| FieldPath::parse(root, text))
        .transpose()?;
    let mut roots = if let Some(registry_id) = args.registry_id {
        live::discover(ctx, registry_id).await?
    } else {
        args.account_id
    };
    roots.sort_unstable();
    roots.dedup();
    let scanned = roots.len();
    let mut loader = live::Loader::new(ctx, &args.sources, args.concurrency as usize);
    roots = loader.classify(roots, root).await;
    for predicate in &predicates {
        let values = loader
            .values(&roots, root, std::slice::from_ref(&predicate.path))
            .await;
        roots.retain(|id| predicate.matches(values[id][&predicate.path].as_ref()));
        if roots.is_empty() {
            break;
        }
    }
    let matched = roots.len();
    if let Some(path) = sort {
        let values = loader
            .values(&roots, root, std::slice::from_ref(&path))
            .await;
        roots.sort_by(|left, right| {
            compare_values(
                values[left][&path].as_ref(),
                values[right][&path].as_ref(),
                args.descending,
            )
            .then_with(|| left.cmp(right))
        });
    }
    let targets = if let Some(path) = mapping {
        let targets = loader.targets(&roots, root, &path).await?;
        let mut seen = BTreeSet::new();
        roots
            .into_iter()
            .filter_map(|id| {
                let target = targets[&id].as_ref()?;
                seen.insert(target).then(|| target.clone())
            })
            .collect::<Vec<_>>()
    } else {
        roots
    };
    let targets = if let Some(limit) = args.limit {
        targets.into_iter().take(limit as usize).collect::<Vec<_>>()
    } else {
        targets
    };
    let mut values = loader.values(&targets, output_kind, &selected).await;
    let rows = targets
        .iter()
        .map(|id| {
            values
                .remove(id)
                .map(output::row)
                .context("loader omitted a requested account")
        })
        .collect::<anyhow::Result<_>>()?;
    let failures = loader.failures;
    if !failures.is_empty() {
        tracing::warn!(
            failures = failures.len(),
            "Query is incomplete; see failures in JSON output"
        );
    }
    print_json(&QueryResult {
        scanned,
        matched,
        complete: failures.is_empty(),
        rows,
        failures,
    })
    .context("print query results")
}
