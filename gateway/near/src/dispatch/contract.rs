use blockchain_gateway_core::contract;
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
            let params = params.0.params;
            let value = client
                .contract(params.contract_id.clone())
                .view_function(&params.method_name.0, params.args.try_into_bytes()?)
                .await?;

            Ok(contract::ViewFunctionResult { value })
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
                parsed: version_string
                    .parse::<blockchain_gateway_core::Version<()>>()
                    .ok(),
                version_string,
            })
        })
    }
}
