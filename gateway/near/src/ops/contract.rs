use std::io::ErrorKind;

use blockchain_gateway_core::{contract, Version};
use near_account_id::AccountId;

use crate::{GatewayResult, NearClient};

pub async fn version<T>(client: &NearClient, contract_id: AccountId) -> GatewayResult<Version<T>> {
    let metadata = client
        .contract(contract_id)
        .contract_source_metadata(())
        .await?;
    let Some(version_string) = metadata.version else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "contract metadata does not contain version",
        )
        .into());
    };

    Ok(version_string
        .parse()
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?)
}

pub async fn get_version(
    client: &NearClient,
    params: contract::GetVersionParams,
) -> GatewayResult<contract::VersionResult> {
    let parsed = version::<()>(client, params.contract_id).await?;
    let version_string = parsed.to_string();

    Ok(contract::VersionResult {
        parsed: Some(parsed),
        version_string,
    })
}
