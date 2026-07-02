use std::{borrow::Borrow, io::ErrorKind};

use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use templar_common::registry::DeployMode;
use templar_gateway_types::{
    common::{ContractArgs, Pagination},
    Base64Bytes, ContractMethodName, RegistryVersion,
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
        pub fn list_deployments(Pagination) -> Vec<AccountId>;
        pub fn list_versions(Pagination) -> Vec<String>;
    }

    pub fn add_version(
        &self,
        options: ContractWriteOptions,
        registry_version: RegistryVersion,
        args: impl Borrow<AddVersionArgs>,
    ) -> GatewayResult<PlannedTransaction> {
        let args = args.borrow();
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

    pub fn deploy(
        &self,
        options: ContractWriteOptions,
        registry_version: RegistryVersion,
        args: impl Borrow<DeployArgs>,
    ) -> GatewayResult<PlannedTransaction> {
        let method_name = registry_version.deploy_method_name();
        Ok(PlannedTransaction {
            signer_account_id: options.signer_account_id,
            receiver_id: self.contract_id().to_owned(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: ContractMethodName(method_name.to_string()).0,
                args: ContractArgs::Json(serde_json::to_value(args.borrow())?).try_into_bytes()?,
                gas: options.gas,
                deposit: options.deposit,
            }))],
        })
    }

    contract_writes! {
        pub fn remove_version(RemoveVersionArgs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBLIC_KEY: &str = "ed25519:5BGSaf6YjVm7565VzWQHNxoyEjwr3jUpRJSGjREvU9dB";

    /// The wire form of `DeployArgs.full_access_keys` must be the
    /// `"ed25519:<bs58>"` string the registry contract's `near_sdk::PublicKey`
    /// parameter deserializes from. This guards the gateway serialization path
    /// against a `near_api`-side format regression (ENG-404).
    #[test]
    fn deploy_args_serializes_full_access_keys_as_curve_prefixed_string() {
        let key: near_api::PublicKey = TEST_PUBLIC_KEY.parse().unwrap();
        let args = DeployArgs {
            name: "market".to_owned(),
            version_key: "market@0.0.0".to_owned(),
            init_args: Base64Bytes(b"{}".to_vec()),
            full_access_keys: Some(vec![key]),
        };

        let value = serde_json::to_value(&args).unwrap();
        assert_eq!(
            value["full_access_keys"],
            serde_json::json!([TEST_PUBLIC_KEY]),
        );
    }

    /// The gateway path (`near_api::PublicKey`) and the direct near-workspaces
    /// path (`near_sdk::PublicKey`) must emit byte-identical JSON, otherwise the
    /// contract would receive a key it cannot parse.
    #[test]
    fn near_api_and_near_sdk_public_key_json_match() {
        let near_api_key: near_api::PublicKey = TEST_PUBLIC_KEY.parse().unwrap();
        let near_sdk_key: near_sdk::PublicKey = TEST_PUBLIC_KEY.parse().unwrap();
        assert_eq!(
            serde_json::to_string(&near_api_key).unwrap(),
            serde_json::to_string(&near_sdk_key).unwrap(),
        );
    }
}
