use near_account_id::AccountId;
use templar_universal_account::{KeyParameters, PayloadExecutionParameters};

use crate::{
    client::{macros::contract_writes, NearClient},
    GatewayResult, ReadNear,
};

use super::BoundContractClient;

#[derive(serde::Serialize)]
pub struct UaGetKeyArgs {
    pub key: templar_universal_account::KeyId,
}

/// The user's signed `execute` payload, forwarded to the contract verbatim.
#[derive(serde::Serialize)]
pub struct UaExecuteArgs {
    pub args: serde_json::Value,
}

#[derive(Clone)]
pub struct UniversalAccountClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: AccountId,
}

impl BoundContractClient for UniversalAccountClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl UniversalAccountClient<'_> {
    contract_writes! {
        pub fn execute(UaExecuteArgs);
    }

    /// Read a key's execution parameters, normalizing the contract's *versioned*
    /// `get_key` response into [`PayloadExecutionParameters`].
    ///
    /// Older contract versions return bare [`KeyParameters`]; newer ones return
    /// the full [`PayloadExecutionParameters`]. Handling that here means every
    /// gateway consumer gets the upgrade for free, rather than re-implementing it
    /// (and risking dropping legacy-key support).
    pub async fn get_key(
        &self,
        args: UaGetKeyArgs,
    ) -> GatewayResult<Option<PayloadExecutionParameters>> {
        let versioned: Option<VersionedKeyParameters> = <NearClient as ReadNear>::view_function(
            self.inner,
            self.contract_id.clone(),
            "get_key",
            serde_json::to_vec(&args)?,
        )
        .await?;
        Ok(versioned.map(|versioned| versioned.into_parameters(&self.contract_id)))
    }
}

/// The contract's versioned `get_key` response.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum VersionedKeyParameters {
    /// Newer contracts return the full parameters.
    V1(PayloadExecutionParameters),
    /// Older contracts return bare key parameters, upgraded using the account id
    /// as the verifying contract.
    V0(KeyParameters),
}

impl VersionedKeyParameters {
    fn into_parameters(self, account_id: &AccountId) -> PayloadExecutionParameters {
        match self {
            Self::V1(parameters) => parameters,
            Self::V0(key_parameters) => PayloadExecutionParameters::builder_empty()
                .with_key_parameters(key_parameters)
                .verifying_contract(account_id.clone())
                .build(),
        }
    }
}
