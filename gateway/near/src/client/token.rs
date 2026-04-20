use blockchain_gateway_core::U128;
use templar_common::asset::{AssetClass, FungibleAsset};

use crate::{GatewayResult, NearClient};

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

    pub async fn transfer_call(
        &self,
        options: ContractWriteOptions,
        receiver_id: near_account_id::AccountId,
        amount: impl Into<u128>,
        msg: String,
    ) -> GatewayResult<near_api::types::transaction::result::TransactionResult> {
        let amount = U128(amount.into());

        match self {
            Self::Ft(client) => {
                client
                    .ft_transfer_call(
                        options,
                        FtTransferCallArgs {
                            receiver_id,
                            amount,
                            msg,
                        },
                    )
                    .await
            }
            Self::Mt { client, token_id } => {
                client
                    .mt_transfer_call(
                        options,
                        MtTransferCallArgs {
                            receiver_id,
                            token_id: token_id.clone(),
                            amount,
                            msg,
                        },
                    )
                    .await
            }
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
