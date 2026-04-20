use std::io::ErrorKind;

use blockchain_gateway_core::{
    common::{ContractArgs, Pagination},
    Base64Bytes, ContractMethodName, RegistryVersion,
};
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use templar_common::registry::DeployMode;

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
    pub account_id: near_account_id::AccountId,
}

#[derive(Debug)]
pub struct AddVersionArgs {
    pub version_key: String,
    pub mode: templar_common::registry::DeployMode,
    pub code: Vec<u8>,
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

#[derive(Clone)]
pub struct RegistryClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: blockchain_gateway_core::RegistryId,
}

impl BoundContractClient for RegistryClient<'_> {
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id.0
    }

    fn client(&self) -> &NearClient {
        self.inner
    }
}

impl RegistryClient<'_> {
    contract_views! {
        pub fn get_deployment(GetDeploymentArgs) -> Option<templar_common::registry::Deployment>;
        pub fn list_deployments(Pagination) -> Vec<near_account_id::AccountId>;
        pub fn list_versions(Pagination) -> Vec<String>;
    }

    pub async fn add_version(
        &self,
        options: ContractWriteOptions,
        registry_version: RegistryVersion,
        args: AddVersionArgs,
    ) -> GatewayResult<PlannedTransaction> {
        if args.mode == DeployMode::GlobalHash && !registry_version.supports_global_contracts() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                format!("Registry version {registry_version} does not support global contracts"),
            )
            .into());
        }
        let encoded_args =
            registry_version.encode_add_version_args(&args.version_key, args.mode, &args.code)?;
        Ok(PlannedTransaction {
            signer_account_id: options.signer_account_id,
            receiver_id: self.contract_id().to_owned(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: ContractMethodName("add_version".to_string()).0,
                args: ContractArgs::Raw(encoded_args.into()).try_into_bytes()?,
                gas: options.gas,
                deposit: options.deposit,
            }))],
        })
    }

    pub async fn deploy(
        &self,
        options: ContractWriteOptions,
        registry_version: RegistryVersion,
        args: DeployArgs,
    ) -> GatewayResult<PlannedTransaction> {
        let method_name = registry_version.deploy_method_name();
        Ok(PlannedTransaction {
            signer_account_id: options.signer_account_id,
            receiver_id: self.contract_id().to_owned(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: ContractMethodName(method_name.to_string()).0,
                args: ContractArgs::Json(serde_json::to_value(&args)?).try_into_bytes()?,
                gas: options.gas,
                deposit: options.deposit,
            }))],
        })
    }

    contract_writes! {
        pub fn remove_version(RemoveVersionArgs);
    }
}
