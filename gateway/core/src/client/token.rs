use near_account_id::AccountId;
use near_sdk_contract_tools::mt::{metadata::TokenMetadataAll, Token};
use templar_common::asset::{AssetClass, FungibleAsset};
use templar_gateway_types::TokenMetadata;
use templar_primitives::SU128;

use crate::{operation::PlannedTransaction, GatewayError, GatewayResult, NearClient};

use super::{
    ft::TransferCallArgs as FtTransferCallArgs, mt::TransferCallArgs as MtTransferCallArgs,
    ContractWriteOptions, FtClient, MtClient,
};

pub enum TokenClient<'a> {
    Ft(FtClient<'a>),
    Mt {
        client: MtClient<'a>,
        token_id: String,
    },
}

impl<'a> TokenClient<'a> {
    pub fn new<T: AssetClass>(near: &'a NearClient, asset: FungibleAsset<T>) -> Self {
        if let Some(contract_id) = asset.clone().into_nep141() {
            return Self::Ft(near.ft(contract_id));
        }

        if let Some((contract_id, token_id)) = asset.into_nep245() {
            return Self::Mt {
                client: near.mt(contract_id),
                token_id,
            };
        }

        unreachable!("fungible asset should always be NEP-141 or NEP-245")
    }

    /// Read metadata from the requested token's own standard, without inferring
    /// a wrapped asset from its token ID or display fields.
    pub async fn metadata(&self) -> GatewayResult<TokenMetadata> {
        match self {
            Self::Ft(client) => {
                let metadata = client.ft_metadata(()).await?;
                Ok(TokenMetadata {
                    decimals: metadata.decimals,
                    name: Some(metadata.name),
                    symbol: Some(metadata.symbol),
                })
            }
            Self::Mt { client, token_id } => {
                let token_ids = [token_id.as_str()];
                let args = || super::mt::GetTokensArgs {
                    token_ids: &token_ids,
                };
                require_mt_token(client.mt_token(args()).await?, token_id)?;
                metadata_from_mt(client.mt_metadata_token_all(args()).await?)
            }
        }
    }

    pub fn transfer_call(
        &self,
        options: ContractWriteOptions,
        receiver_id: AccountId,
        amount: impl Into<u128>,
        msg: String,
    ) -> GatewayResult<PlannedTransaction> {
        let amount = SU128::from(amount.into());

        match self {
            Self::Ft(client) => client.ft_transfer_call(
                options,
                FtTransferCallArgs {
                    receiver_id,
                    amount,
                    memo: None,
                    msg,
                },
            ),
            Self::Mt { client, token_id } => client.mt_transfer_call(
                options,
                MtTransferCallArgs {
                    receiver_id,
                    token_id: token_id.clone(),
                    amount,
                    approval: None,
                    memo: None,
                    msg,
                },
            ),
        }
    }

    pub fn contract_id(&self) -> &near_account_id::AccountIdRef {
        match self {
            Self::Ft(client) => &client.contract_id,
            Self::Mt { client, .. } => &client.contract_id,
        }
    }

    pub fn token_id(&self) -> Option<&str> {
        match self {
            Self::Ft(_) => None,
            Self::Mt { token_id, .. } => Some(token_id),
        }
    }
}

fn require_mt_token(tokens: Vec<Option<Token>>, token_id: &str) -> GatewayResult<()> {
    let [token]: [_; 1] = tokens.try_into().map_err(|_| {
        GatewayError::NearQuery("mt_token must return exactly one token".to_owned())
    })?;
    match token {
        Some(token) if token.token_id == token_id => Ok(()),
        _ => Err(GatewayError::NearQuery(format!(
            "mt_token did not return the requested token {token_id:?}"
        ))),
    }
}

fn metadata_from_mt(metadata: Vec<TokenMetadataAll>) -> GatewayResult<TokenMetadata> {
    let [metadata]: [_; 1] = metadata.try_into().map_err(|_| {
        GatewayError::NearQuery("mt_metadata_token_all must return exactly one result".to_owned())
    })?;
    let decimals = metadata
        .base
        .decimals
        .ok_or_else(|| GatewayError::NearQuery("MT metadata has no decimals".to_owned()))?
        .0;
    let decimals = u8::try_from(decimals).map_err(|_| {
        GatewayError::NearQuery(format!("MT metadata decimals {decimals} exceed u8"))
    })?;
    Ok(TokenMetadata {
        decimals,
        name: Some(metadata.base.name),
        symbol: metadata.base.symbol,
    })
}

#[cfg(test)]
mod tests {
    use near_api::NetworkConfig;
    use near_sdk::json_types::U64;
    use near_sdk_contract_tools::mt::metadata::{BaseMetadata, TokenMetadata as MtTokenMetadata};
    use serde_json::{json, Value};
    use templar_common::asset::BorrowAsset;
    use wiremock::{
        matchers::{body_partial_json, method},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;

    fn token(token_id: &str) -> Token {
        Token {
            token_id: token_id.to_owned(),
            owner_id: None,
            extensions_metadata: std::collections::HashMap::new(),
        }
    }

    fn metadata(decimals: Option<u64>) -> TokenMetadataAll {
        let mut base = BaseMetadata::new("Opaque token", "shared-base");
        base.decimals = decimals.map(U64);
        TokenMetadataAll {
            base,
            token: MtTokenMetadata::default(),
        }
    }

    fn response(value: &impl serde::Serialize) -> serde_json::Result<Value> {
        Ok(json!({
            "jsonrpc": "2.0", "id": "0",
            "result": {
                "block_hash": "11111111111111111111111111111111",
                "block_height": 1, "logs": [],
                "result": serde_json::to_vec(value)?,
            },
        }))
    }

    #[tokio::test]
    async fn metadata_rejects_unknown_or_ambiguous_mt_responses() -> anyhow::Result<()> {
        let id = "nep245:wrapper.near:chain:asset";
        for (tokens, metadata, expected_decimals) in [
            (vec![], None, None),
            (vec![None], None, None),
            (vec![Some(token("nep245:wrapper.near:chain"))], None, None),
            (vec![Some(token(id)), Some(token(id))], None, None),
            (vec![Some(token(id))], Some(vec![]), None),
            (vec![Some(token(id))], Some(vec![metadata(None)]), None),
            (vec![Some(token(id))], Some(vec![metadata(Some(256))]), None),
            (
                vec![Some(token(id))],
                Some(vec![metadata(Some(u64::MAX))]),
                None,
            ),
            (
                vec![Some(token(id))],
                Some(vec![metadata(Some(24)), metadata(Some(24))]),
                None,
            ),
            (
                vec![Some(token(id))],
                Some(vec![metadata(Some(255))]),
                Some(255),
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(body_partial_json(
                    json!({"params": {"method_name": "mt_token"}}),
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(response(&tokens)?))
                .expect(1)
                .mount(&server)
                .await;
            let metadata_calls = u64::from(metadata.is_some());
            Mock::given(method("POST"))
                .and(body_partial_json(json!({
                    "params": {"method_name": "mt_metadata_token_all"}
                })))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(response(&metadata.unwrap_or_default())?),
                )
                .expect(metadata_calls)
                .mount(&server)
                .await;
            let near = NearClient::new(NetworkConfig::from_rpc_url("test", server.uri().parse()?));
            let asset =
                FungibleAsset::<BorrowAsset>::nep245("mt.test.near".parse()?, id.to_owned());
            let result = near.token(asset).metadata().await;
            match expected_decimals {
                Some(decimals) => assert_eq!(result?.decimals, decimals),
                None => assert!(matches!(result, Err(GatewayError::NearQuery(_)))),
            }
        }
        Ok(())
    }
}
