use blockchain_gateway_core::universal_account;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearClient, actor::{DispatchRead, RpcMessage}};

fn into_parameters_view(
    parameters: templar_universal_account::PayloadExecutionParameters,
) -> universal_account::PayloadExecutionParametersView {
    universal_account::PayloadExecutionParametersView {
        block_height: parameters.block_height.0,
        index: parameters.index.0,
        nonce: parameters.nonce.0,
        name: parameters.name,
        version: parameters.version,
        chain_id: parameters.chain_id.map(|value| value.0),
        verifying_contract: parameters
            .verifying_contract
            .to_string()
            .parse()
            .expect("templar universal account should emit valid account ids"),
        salt: parameters
            .salt
            .and_then(|value| serde_json::to_value(value).ok())
            .and_then(|value| value.as_str().map(str::to_owned)),
    }
}

impl DispatchRead for universal_account::GetKey {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .universal_account(params.account_id)
                .get_key(params.args)
                .await
                .map(|parameters| universal_account::GetKeyResult {
                    parameters: parameters.map(into_parameters_view),
                })
        })
    }
}
