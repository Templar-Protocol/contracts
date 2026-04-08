use blockchain_gateway_core::universal_account;
use templar_universal_account::PayloadExecutionParameters;

use crate::{GatewayResult, GatewayService};

fn convert_account_id(account_id: impl ToString) -> near_account_id::AccountId {
    account_id
        .to_string()
        .parse()
        .expect("templar universal account should emit valid account ids")
}

fn into_parameters_view(
    parameters: PayloadExecutionParameters,
) -> universal_account::PayloadExecutionParametersView {
    universal_account::PayloadExecutionParametersView {
        block_height: parameters.block_height.0,
        index: parameters.index.0,
        nonce: parameters.nonce.0,
        name: parameters.name,
        version: parameters.version,
        chain_id: parameters.chain_id.map(|value| value.0),
        verifying_contract: parameters.verifying_contract,
        salt: parameters
            .salt
            .and_then(|value| serde_json::to_value(value).ok())
            .and_then(|value| value.as_str().map(str::to_owned)),
    }
}

pub async fn get_key(
    service: &GatewayService,
    params: universal_account::GetKeyParams,
) -> GatewayResult<universal_account::GetKeyResult> {
    let parameters = service
        .near()
        .universal_account(params.account_id)
        .get_key(params.args)
        .await?
        .map(into_parameters_view);

    Ok(universal_account::GetKeyResult { parameters })
}
