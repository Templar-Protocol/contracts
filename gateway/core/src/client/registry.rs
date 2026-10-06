use std::borrow::Borrow;

use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use near_sdk::json_types::Base58CryptoHash;
use templar_common::registry::VersionSource;
use templar_gateway_types::{
    common::{ContractArgs, Pagination},
    Base64Bytes, ContractMethodName,
};

use crate::{
    client::{
        macros::{contract_views, contract_writes},
        NearClient,
    },
    operation::PlannedTransaction,
    GatewayResult,
};

use super::{BoundContractClient, ContractWriteOptions};

#[derive(Debug, serde::Serialize)]
pub struct GetDeploymentArgs {
    pub account_id: AccountId,
}

#[derive(Debug, serde::Serialize)]
pub struct GetRegistryEntryArgs {
    pub account_id: AccountId,
}

#[derive(Debug, serde::Serialize)]
pub struct GetVersionArgs {
    pub version_key: String,
}

#[derive(Debug, serde::Serialize)]
pub struct GetVersionCodeChunkArgs {
    pub version_key: String,
    pub offset: u32,
    pub len: u32,
}

#[derive(Debug)]
pub struct AddVersionArgs {
    pub version_key: String,
    pub source: VersionSource,
}

#[derive(Debug, serde::Serialize)]
pub struct DeployArgs {
    pub name: String,
    pub version_key: String,
    pub init_args: Base64Bytes,
    pub full_access_keys: Option<Vec<near_api::PublicKey>>,
}

#[derive(Debug, serde::Serialize)]
pub struct RemoveVersionArgs {
    pub version_key: String,
}

#[derive(Debug, serde::Serialize)]
pub struct UpgradeArgs {
    pub code: templar_common::upgrade::UpgradeSource,
    pub migrate_args: Base64Bytes,
}

#[derive(Clone)]
pub struct RegistryClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: AccountId,
}

impl BoundContractClient for RegistryClient<'_> {
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }

    fn client(&self) -> &NearClient {
        self.inner
    }
}

impl RegistryClient<'_> {
    contract_views! {
        pub fn get_deployment(GetDeploymentArgs) -> Option<templar_common::registry::Deployment>;
        pub fn get_registry_entry(GetRegistryEntryArgs) -> Option<templar_common::registry::RegistryEntryView>;
        pub fn get_version(GetVersionArgs) -> Option<templar_common::registry::VersionInfo>;
        pub fn get_version_code_hash(GetVersionArgs) -> Option<Base58CryptoHash>;
        pub fn list_deployments(Pagination) -> Vec<AccountId>;
        pub fn list_versions(Pagination) -> Vec<String>;
    }

    pub async fn get_version_code_chunk(
        &self,
        args: impl Borrow<GetVersionCodeChunkArgs>,
    ) -> GatewayResult<Option<Vec<u8>>> {
        crate::ReadNear::view_function_borsh(
            self.inner,
            self.contract_id.clone(),
            "get_version_code_chunk",
            serde_json::to_vec(args.borrow())?,
        )
        .await
    }

    pub fn add_version(
        &self,
        options: ContractWriteOptions,
        args: impl Borrow<AddVersionArgs>,
    ) -> GatewayResult<PlannedTransaction> {
        let args = args.borrow();
        let encoded_args = borsh::to_vec(&(&args.version_key, &args.source))?;
        Ok(PlannedTransaction {
            signer_account_id: options.signer_account_id,
            receiver_id: self.contract_id().to_owned(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: ContractMethodName("add_version".to_string()).0,
                args: ContractArgs::Raw(encoded_args.into()).try_into_bytes()?,
                gas: options.gas,
                deposit: options.deposit,
            }))],
            continue_on_failure: false,
        })
    }

    pub fn deploy(
        &self,
        options: ContractWriteOptions,
        args: impl Borrow<DeployArgs>,
    ) -> GatewayResult<PlannedTransaction> {
        Ok(PlannedTransaction {
            signer_account_id: options.signer_account_id,
            receiver_id: self.contract_id().to_owned(),
            continue_on_failure: false,
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: ContractMethodName("deploy".to_string()).0,
                args: ContractArgs::Json(serde_json::to_value(args.borrow())?).try_into_bytes()?,
                gas: options.gas,
                deposit: options.deposit,
            }))],
        })
    }

    contract_writes! {
        pub fn remove_version(RemoveVersionArgs);
        pub fn upgrade(UpgradeArgs);
    }
}

#[cfg(test)]
mod tests {
    use near_api::{types::transaction::actions::Action, NetworkConfig};
    use near_sdk::json_types::{Base58CryptoHash, Base64VecU8};
    use templar_common::registry::VersionSource;
    use templar_gateway_types::ManagedAccountId;

    use super::{AddVersionArgs, NearClient};
    use crate::client::ContractWriteOptions;

    const KEY: &str = "market@1.5.0";
    const CODE: [u8; 3] = [0xde, 0xad, 0xbe];

    #[rstest::rstest]
    #[case::stored(VersionSource::Stored(Base64VecU8(CODE.to_vec())))]
    #[case::publish_global(VersionSource::PublishGlobal(Base64VecU8(CODE.to_vec())))]
    #[case::existing_global(VersionSource::ExistingGlobal(Base58CryptoHash::from([7u8; 32])))]
    fn add_version_sends_the_borsh_key_and_source(#[case] source: VersionSource) {
        let client = NearClient::new(NetworkConfig::from_rpc_url(
            "test",
            "http://127.0.0.1:1".parse().unwrap(),
        ));
        let planned = client
            .registry("registry.near".parse().unwrap())
            .add_version(
                ContractWriteOptions::new(ManagedAccountId("owner.near".parse().unwrap()))
                    .one_yocto(),
                AddVersionArgs {
                    version_key: KEY.to_owned(),
                    source: source.clone(),
                },
            )
            .unwrap();

        let [Action::FunctionCall(action)] = &planned.actions[..] else {
            panic!("expected one function call");
        };
        assert_eq!(action.method_name, "add_version");
        assert_eq!(action.args, borsh::to_vec(&(KEY, &source)).unwrap());
    }
}
