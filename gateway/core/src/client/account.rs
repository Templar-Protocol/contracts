use templar_gateway_types::NearToken;

use crate::{client::NearClient, GatewayError, GatewayResult, ReadNear};

#[derive(Clone, Copy)]
pub struct AccountClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl AccountClient<'_> {
    pub async fn get(
        &self,
        account_id: near_account_id::AccountId,
    ) -> GatewayResult<near_api::types::account::Account> {
        <NearClient as ReadNear>::view_account(self.inner, account_id).await
    }

    /// Liquid NEAR remaining after storage backing not covered by locked stake.
    pub async fn spendable_balance(
        &self,
        account_id: near_account_id::AccountId,
    ) -> GatewayResult<NearToken> {
        let account = self.get(account_id).await?;
        let byte_cost = self.inner.chain().storage_byte_cost().await?;
        liquid_after_storage(
            account.amount,
            account.locked,
            account.storage_usage,
            byte_cost,
        )
    }

    pub async fn access_key(
        &self,
        account_id: near_account_id::AccountId,
        public_key: near_api::types::PublicKey,
    ) -> GatewayResult<near_api::types::transaction::actions::AccessKey> {
        <NearClient as ReadNear>::view_access_key(self.inner, account_id, public_key).await
    }

    pub async fn access_keys(
        &self,
        account_id: near_account_id::AccountId,
    ) -> GatewayResult<
        Vec<(
            near_api::types::PublicKey,
            near_api::types::transaction::actions::AccessKey,
        )>,
    > {
        <NearClient as ReadNear>::view_access_key_list(self.inner, account_id).await
    }
}

fn liquid_after_storage(
    amount: NearToken,
    locked: NearToken,
    storage_usage: u64,
    byte_cost: NearToken,
) -> GatewayResult<NearToken> {
    let required = byte_cost
        .as_yoctonear()
        .checked_mul(u128::from(storage_usage))
        .ok_or_else(|| {
            GatewayError::NearQuery("account storage backing overflows u128".to_owned())
        })?;
    let liquid_backing = required.saturating_sub(locked.as_yoctonear());
    Ok(NearToken::from_yoctonear(
        amount.as_yoctonear().saturating_sub(liquid_backing),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spendable_near_reserves_only_storage_not_already_backed_by_locked_stake() -> GatewayResult<()>
    {
        for (amount, locked, usage, price, expected) in [
            (50, 0, 4, 5, 30),
            (50, 10, 4, 5, 40),
            (50, 20, 4, 5, 50),
            (50, 25, 4, 5, 50),
            (10, 0, 4, 5, 0),
            (50, 0, 0, 5, 50),
        ] {
            assert_eq!(
                liquid_after_storage(
                    NearToken::from_yoctonear(amount),
                    NearToken::from_yoctonear(locked),
                    usage,
                    NearToken::from_yoctonear(price)
                )?
                .as_yoctonear(),
                expected
            );
        }
        assert!(liquid_after_storage(
            NearToken::from_yoctonear(50),
            NearToken::from_yoctonear(0),
            2,
            NearToken::from_yoctonear(u128::MAX)
        )
        .is_err());
        Ok(())
    }
}
