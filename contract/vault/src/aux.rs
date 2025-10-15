use crate::{env, near, serde_json, AccountId, Contract, Nep145Controller, Nep145ForceUnregister};

impl Contract {
    /* ----- Storage ----- */
    fn charge_for_storage(&mut self, account_id: &AccountId, storage_consumption: u64) {
        // Invariant: Storage charging saturates and panics on failure to avoid negative balances.
        self.lock_storage(
            account_id,
            env::storage_byte_cost().saturating_mul(u128::from(storage_consumption)),
        )
        .unwrap_or_else(|e| env::panic_str(&format!("Storage error: {e}")));
    }

    fn refund_for_storage(&mut self, account_id: &AccountId, storage_consumption: u64) {
        // Invariant: Storage refunds saturate and panic on failure to preserve accounting integrity.
        self.unlock_storage(
            account_id,
            env::storage_byte_cost().saturating_mul(u128::from(storage_consumption)),
        )
        .unwrap_or_else(|e| env::panic_str(&format!("Storage error: {e}")));
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
/// Indicates the JSON return payload shape expected by token receiver callbacks.
pub enum ReturnStyle {
    /// Return payload shape for NEP-141 `ft_transfer_call` (a bare amount).
    Nep141FtTransferCall,
    /// Return payload shape for NEP-245 `mt_transfer_call` (a single-element array).
    Nep245MtTransferCall,
}

// TODO: use this
impl ReturnStyle {
    #[must_use]
    pub fn serialize(
        &self,
        amount: templar_common::asset::FungibleAssetAmount<impl templar_common::asset::AssetClass>,
    ) -> serde_json::Value {
        match self {
            Self::Nep141FtTransferCall => serde_json::json!(amount),
            Self::Nep245MtTransferCall => serde_json::json!([amount]),
        }
    }
}

impl near_sdk_contract_tools::hook::Hook<Self, Nep145ForceUnregister<'_>> for Contract {
    fn hook<R>(_: &mut Self, _: &Nep145ForceUnregister, _: impl FnOnce(&mut Self) -> R) -> R {
        // Invariant: Force unregister must fail to preserve FT ledger integrity.
        env::panic_str("force unregistration is not supported")
    }
}
