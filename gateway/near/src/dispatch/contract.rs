use blockchain_gateway_core::contract;
use futures::future::BoxFuture;

use crate::{actor::DispatchRead, GatewayResult, NearClient};

impl DispatchRead for contract::ViewFunction {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let value = client
                .contract(request.params.contract_id.clone())
                .view_function(
                    &request.params.method_name.0,
                    request.params.args.try_into_bytes()?,
                )
                .await?;

            Ok(contract::ViewFunctionResult { value })
        })
    }
}

impl DispatchRead for contract::GetVersion {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let metadata = client
                .contract(request.params.contract_id)
                .contract_source_metadata(())
                .await?;
            let version_string = metadata.version.ok_or_else(|| {
                crate::GatewayError::NearQuery(
                    "contract metadata does not contain version".to_owned(),
                )
            })?;

            Ok(contract::VersionResult {
                parsed: version_string.parse().ok(),
                version_string,
            })
        })
    }
}
