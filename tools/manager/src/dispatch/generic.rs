//! The `read`/`write <method> --json` escape hatch: invoke any gateway method by
//! its RPC name with raw JSON params, for methods without a typed command.

use anyhow::Context as _;
use serde_json::Value;
use std::io::Read as _;
use templar_gateway_oracle_updates_spec::oracle as oracle_spec;
use templar_gateway_types::MethodSpec;

use crate::cli::{GenericMethodCall, WriteMethodCall};
use crate::context::{all_sources, lazer_source, pyth_source, redstone_source, CliContext};

pub(super) async fn read(ctx: CliContext, call: GenericMethodCall) -> anyhow::Result<()> {
    let method = call.method.clone();
    let params = load_params(call)?;

    macro_rules! try_read {
        ($spec:ty) => {
            if method == <$spec as MethodSpec>::RPC_METHOD {
                let request: $spec = serde_json::from_value(params)
                    .with_context(|| format!("parse parameters for {method}"))?;
                return ctx.read(request).await;
            }
        };
    }
    templar_gateway_methods_spec::for_each_read_method!(try_read);
    anyhow::bail!("unsupported read method {method}");
}

/// The `oracle.*` update a method name selects, and with it the payload sources its
/// context must carry. `None` for every other write, which the methods dispatcher serves.
///
/// Spelled out rather than expanded from
/// [`for_each_oracle_update_method!`](templar_gateway_oracle_updates_spec::for_each_oracle_update_method)
/// because each variant needs a differently-typed context, which one macro callback
/// cannot produce. `every_oracle_update_method_is_routed` closes the drift that opens up.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OracleRoute {
    Pyth,
    RedStone,
    Lazer,
    Prices,
}

pub(crate) fn oracle_route(method: &str) -> Option<OracleRoute> {
    match method {
        _ if method == oracle_spec::UpdatePyth::RPC_METHOD => Some(OracleRoute::Pyth),
        _ if method == oracle_spec::UpdateRedStone::RPC_METHOD => Some(OracleRoute::RedStone),
        _ if method == oracle_spec::UpdateLazer::RPC_METHOD => Some(OracleRoute::Lazer),
        _ if method == oracle_spec::UpdatePrices::RPC_METHOD => Some(OracleRoute::Prices),
        _ => None,
    }
}

pub(super) async fn write(ctx: CliContext, call: WriteMethodCall) -> anyhow::Result<()> {
    let WriteMethodCall {
        call,
        oracle_sources,
        signer,
    } = call;
    let method = call.method.clone();
    let params = load_params(call)?;

    /// Deserialize `params` into the spec `method` names.
    macro_rules! body {
        ($spec:ty) => {
            serde_json::from_value::<$spec>(params)
                .with_context(|| format!("parse parameters for {method}"))?
        };
    }

    if let Some(route) = oracle_route(&method) {
        let network = ctx.network();
        return match route {
            OracleRoute::Pyth => {
                ctx.oracle_write(signer, body!(oracle_spec::UpdatePyth), |base| {
                    Ok(pyth_source(base, &oracle_sources.pyth, network))
                })
                .await
            }
            OracleRoute::RedStone => {
                ctx.oracle_write(signer, body!(oracle_spec::UpdateRedStone), |base| {
                    redstone_source(base, &oracle_sources.redstone)
                })
                .await
            }
            OracleRoute::Lazer => {
                ctx.oracle_write(signer, body!(oracle_spec::UpdateLazer), |base| {
                    lazer_source(base, &oracle_sources.lazer)
                })
                .await
            }
            OracleRoute::Prices => {
                ctx.oracle_write(signer, body!(oracle_spec::UpdatePrices), |base| {
                    all_sources(base, &oracle_sources, network)
                })
                .await
            }
        };
    }

    macro_rules! try_write {
        ($spec:ty) => {
            if method == <$spec as MethodSpec>::RPC_METHOD {
                return ctx.write(signer, body!($spec)).await;
            }
        };
    }
    templar_gateway_methods_spec::for_each_write_method!(try_write);
    anyhow::bail!("unsupported write method {method}");
}

/// Load raw JSON params from `--json`, a `--json-file`, or stdin (`-`).
fn load_params(call: GenericMethodCall) -> anyhow::Result<Value> {
    if let Some(json) = call.json {
        return serde_json::from_str(&json).context("parse --json method parameters");
    }
    if let Some(path) = call.json_file {
        if path == std::path::Path::new("-") {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .context("read JSON parameters from stdin")?;
            return serde_json::from_str(&input).context("parse JSON method parameters");
        }
        let input = std::fs::read_to_string(&path)
            .with_context(|| format!("read JSON parameters from {}", path.display()))?;
        return serde_json::from_str(&input).context("parse JSON method parameters");
    }
    anyhow::bail!("missing method parameters (use --json or --json-file)")
}
