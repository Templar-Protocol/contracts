use crate::{env, near, serde_json};

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
