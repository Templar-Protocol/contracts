use blockchain_gateway_core::contract;
use near_contract_standards::contract_metadata::ContractSourceMetadata;

use crate::{
    client::{macros::contract_views, NearClient},
    GatewayResult,
};

use super::BoundContractClient;

#[derive(Clone)]
pub struct ContractClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for ContractClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl ContractClient<'_> {
    pub async fn view_function(
        &self,
        params: contract::ViewFunctionParams,
    ) -> GatewayResult<contract::ViewFunctionResult> {
        let result = self
            .inner
            .view_value(params.contract_id, &params.method_name.0, &params.args)
            .await?;

        Ok(contract::ViewFunctionResult { value: result.data })
    }

    contract_views! {
        pub fn contract_source_metadata(()) -> ContractSourceMetadata;
    }
}
