//! The `read`/`write <method> --json` escape hatch and ordered `read-batch`:
//! invoke gateway methods by their RPC names with raw JSON params.

use anyhow::Context as _;
use futures::{stream, StreamExt as _, TryStreamExt as _};
use serde::Deserialize;
use serde_json::Value;
use templar_gateway_oracle_updates_spec::oracle as oracle_spec;
use templar_gateway_types::MethodSpec;

use crate::cli::{GenericMethodCall, ReadBatchCall, WriteMethodCall};
use crate::context::{
    all_sources, lazer_source, print_json, pyth_source, redstone_source, CliContext,
};

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadBatchItem {
    method: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    params: Value,
}

impl ReadBatchItem {
    fn validate_method(&self) -> anyhow::Result<()> {
        macro_rules! try_read {
            ($spec:ty) => {
                if self.method == <$spec as MethodSpec>::RPC_METHOD {
                    return Ok(());
                }
            };
        }
        templar_gateway_methods_spec::for_each_read_method!(try_read);
        anyhow::bail!("unsupported read method {}", self.method);
    }

    async fn execute(self, ctx: &CliContext) -> anyhow::Result<Value> {
        let Self { method, params } = self;
        macro_rules! try_read {
            ($spec:ty) => {
                if method == <$spec as MethodSpec>::RPC_METHOD {
                    let request: $spec = serde_json::from_value(params)
                        .with_context(|| format!("parse parameters for {method}"))?;
                    return Ok(serde_json::to_value(ctx.client.read(request).await?)?);
                }
            };
        }
        templar_gateway_methods_spec::for_each_read_method!(try_read);
        anyhow::bail!("unsupported read method {method}");
    }
}

pub(super) async fn read_batch(ctx: CliContext, call: ReadBatchCall) -> anyhow::Result<()> {
    let calls: Vec<ReadBatchItem> = load_json(call.json, call.json_file)?;
    // Reject unknown/write methods throughout the batch before starting any read.
    for (index, call) in calls.iter().enumerate() {
        call.validate_method()
            .with_context(|| format!("read-batch item {index} ({})", call.method))?;
    }
    let output: Vec<Value> = stream::iter(calls.into_iter().enumerate())
        .map(|(index, call)| {
            let ctx = &ctx;
            async move {
                call.execute(ctx)
                    .await
                    .with_context(|| format!("read-batch item {index}"))
            }
        })
        .buffered(4)
        .try_collect()
        .await?;
    print_json(&output)
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
    load_json(call.json, call.json_file)
}

fn load_json<T: serde::de::DeserializeOwned>(
    json: Option<String>,
    json_file: Option<std::path::PathBuf>,
) -> anyhow::Result<T> {
    if let Some(json) = json {
        return serde_json::from_str(&json).context("parse --json method parameters");
    }
    if let Some(path) = json_file {
        return crate::commands::load_json_file(&path);
    }
    anyhow::bail!("missing method parameters (use --json or --json-file)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(r#"[{"method":"account.get"}]"#)]
    #[case(r#"[{"params":{}}]"#)]
    #[case(r#"[{"method":"account.get","params":{},"signer_id":"sender.near"}]"#)]
    #[case(r#"[{"method":"account.get","params":{},"method":"account.delete"}]"#)]
    #[case(r#"{"method":"account.get","params":{}}"#)]
    fn batch_rejects_invalid_item_shape(#[case] input: &str) {
        assert!(load_json::<Vec<ReadBatchItem>>(Some(input.to_owned()), None).is_err());
    }

    #[rstest::rstest]
    #[case("account.delete")]
    #[case("oracle.updatePrices")]
    #[case("account.Get")]
    #[case("query")]
    fn batch_rejects_noncanonical_or_write_methods(#[case] method: &str) {
        let call = ReadBatchItem {
            method: method.to_owned(),
            params: serde_json::json!({}),
        };
        assert!(call.validate_method().is_err());
    }
}
