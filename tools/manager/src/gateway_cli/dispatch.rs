use anyhow::Context as _;
use serde::Serialize;
use serde_json::Value;
use templar_gateway_client::Client;
use templar_gateway_types::{common::WriteRequest, MethodSpec};

use super::GatewayCli;

pub(super) async fn dispatch(client: &Client, cli: &GatewayCli) -> anyhow::Result<()> {
    if dispatch_read(client, cli.rpc_method(), cli.params().clone()).await? {
        return Ok(());
    }
    if dispatch_write(client, cli, cli.params().clone()).await? {
        return Ok(());
    }
    anyhow::bail!("unsupported gateway method {}", cli.rpc_method())
}

async fn dispatch_read(client: &Client, rpc_method: &str, params: Value) -> anyhow::Result<bool> {
    if rpc_method == <templar_gateway_methods_spec::op::Get as MethodSpec>::RPC_METHOD {
        let request: templar_gateway_methods_spec::op::Get = serde_json::from_value(params)
            .with_context(|| format!("parse parameters for {rpc_method}"))?;
        let operation = client.operation(&request.operation_id).await?;
        print_json(&templar_gateway_methods_spec::op::GetResult { operation })?;
        return Ok(true);
    }

    macro_rules! try_read {
        ($spec:ty) => {
            if rpc_method == <$spec as MethodSpec>::RPC_METHOD {
                let request: $spec = serde_json::from_value(params)
                    .with_context(|| format!("parse parameters for {rpc_method}"))?;
                let output = client.read(request).await?;
                print_json(&output)?;
                return Ok(true);
            }
        };
    }

    templar_gateway_methods_spec::for_each_read_method!(try_read);
    Ok(false)
}

async fn dispatch_write(client: &Client, cli: &GatewayCli, params: Value) -> anyhow::Result<bool> {
    macro_rules! try_write {
        ($spec:ty) => {
            if cli.rpc_method() == <$spec as MethodSpec>::RPC_METHOD {
                let body: $spec = serde_json::from_value(params)
                    .with_context(|| format!("parse parameters for {}", cli.rpc_method()))?;
                let output = client
                    .execute_request(WriteRequest {
                        signer_account_id: cli.signer_account()?,
                        idempotency_key: cli.idempotency_key(),
                        body,
                    })
                    .await?;
                print_json(&output)?;
                return Ok(true);
            }
        };
    }

    templar_gateway_methods_spec::for_each_write_method!(try_write);
    Ok(false)
}

fn print_json(output: &impl Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, output)?;
    use std::io::Write as _;
    writeln!(lock)?;
    Ok(())
}
