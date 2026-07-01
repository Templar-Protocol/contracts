use near_account_id::AccountId;
use templar_common::asset::{AssetClass, FungibleAsset};
use templar_primitives::SU128;

use crate::{operation::PlannedTransaction, GatewayResult, NearClient};

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
