#![allow(clippy::unwrap_used)]
#![allow(dead_code)]

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use near_sdk::AccountId;
use near_workspaces::{network::Sandbox, Account, Worker};
use serde::{de::DeserializeOwned, Serialize};
use templar_manager::{
    near,
    util::{ContractLoader, GeneralArgsLoader, SignerArgs},
    CliContext,
};

/// Create a [`CliContext`] pointing at the sandbox RPC.
///
pub fn setup_ctx(worker: &Worker<Sandbox>) -> CliContext {
    CliContext {
        transaction_url_prefix: String::new(),
        near: near_jsonrpc_client::JsonRpcClient::connect(worker.rpc_addr()),
    }
}

pub fn workspace_path() -> PathBuf {
    PathBuf::from(env!("CARGO_WORKSPACE_DIR"))
}

pub fn no_build_loader() -> ContractLoader {
    ContractLoader {
        no_build: true,
        workspace_path: workspace_path(),
    }
}

/// Build [`SignerArgs`] from a sandbox [`Account`].
pub fn signer_args(account: &Account) -> SignerArgs {
    SignerArgs::new(
        account.id().clone(),
        account.secret_key().to_string().parse().unwrap(),
    )
}

pub async fn view_json<T: DeserializeOwned>(
    ctx: &CliContext,
    account_id: &AccountId,
    method: &str,
    args: impl Serialize,
) -> T {
    near::view(&ctx.near, account_id, method, args)
        .await
        .unwrap()
}

pub fn write_json_file<T: Serialize>(prefix: &str, value: &T) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("templar-manager-{prefix}-{unique}.json"));
    std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    path
}

#[derive(Clone, Copy)]
pub enum TestArgsKind {
    Inline,
    File,
}

impl TestArgsKind {
    pub fn into_fixture<T: Serialize>(self, prefix: &str, init_args: T) -> TestArgs<T> {
        match self {
            Self::Inline => TestArgs::Inline(init_args),
            Self::File => {
                let file = write_json_file(prefix, &init_args);
                TestArgs::File(file)
            }
        }
    }
}

pub enum TestArgs<T> {
    Inline(T),
    File(PathBuf),
}

impl<T> Drop for TestArgs<T> {
    fn drop(&mut self) {
        if let Self::File(ref file) = self {
            std::fs::remove_file(file).ok();
        }
    }
}

impl<T: Serialize> TestArgs<T> {
    pub fn loader(&self) -> GeneralArgsLoader {
        match self {
            Self::Inline(args) => GeneralArgsLoader::from_json_string(
                serde_json::to_string(&args)
                    .unwrap_or_else(|err| panic!("failed to serialise init args: {err}")),
            ),
            Self::File(path) => GeneralArgsLoader::from_file(path.clone()),
        }
    }
}
