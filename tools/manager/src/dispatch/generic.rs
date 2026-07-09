//! The `read`/`write <method> --json` escape hatch: invoke any gateway method by
//! its RPC name with raw JSON params, for methods without a typed command.

use anyhow::Context as _;
use serde_json::Value;
use std::io::Read as _;
use templar_gateway_types::MethodSpec;

use crate::cli::{GenericMethodCall, WriteMethodCall};
use crate::context::CliContext;

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

pub(super) async fn write(ctx: CliContext, call: WriteMethodCall) -> anyhow::Result<()> {
    let WriteMethodCall { call, signer } = call;
    let method = call.method.clone();
    let params = load_params(call)?;

    macro_rules! try_write {
        ($spec:ty) => {
            if method == <$spec as MethodSpec>::RPC_METHOD {
                let body: $spec = serde_json::from_value(params)
                    .with_context(|| format!("parse parameters for {method}"))?;
                return ctx.write(signer.clone(), body).await;
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
