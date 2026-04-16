use blockchain_gateway_core::{contract, Version};
use futures::future::BoxFuture;

use crate::{
    actor::{DispatchRead, RpcMessage},
    GatewayResult, NearClient,
};

impl DispatchRead for contract::ViewFunction {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .contract(params.0.params.contract_id.clone())
                .view_function(params.0.params)
                .await
        })
    }
}

impl DispatchRead for contract::GetVersion {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let metadata = client
                .contract(params.0.params.contract_id)
                .contract_source_metadata(())
                .await?;
            let version_string = metadata.version.ok_or_else(|| {
                crate::GatewayError::NearQuery(
                    "contract metadata does not contain version".to_owned(),
                )
            })?;

            Ok(contract::VersionResult {
                parsed: version_string.parse::<Version<()>>().ok(),
                version_string,
            })
        })
    }
}
