use near_account_id::AccountId;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FungibleAssetKind {
    Nep141(AccountId),
    Nep245 {
        account_id: AccountId,
        token_id: String,
    },
}

impl FungibleAssetKind {
    pub fn is_nep245(&self, contract_id: &AccountId, token_id: &str) -> bool {
        matches!(self, FungibleAssetKind::Nep245 { account_id: c, token_id: t } if c == contract_id && t == token_id)
    }

    pub fn is_nep141(&self, account_id: &AccountId) -> bool {
        matches!(self, FungibleAssetKind::Nep141(a) if a == account_id)
    }
    
    pub fn account_id(&self) -> &AccountId {
        match self {
            FungibleAssetKind::Nep141(account_id) => account_id,
            FungibleAssetKind::Nep245 { account_id, .. } => account_id,
        }
    }
}

impl FromStr for FungibleAssetKind {
    type Err = near_account_id::ParseAccountError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((contract_id, token_id)) = s.split_once(':') {
            if let Some(token_id) = token_id.strip_prefix("nep245:") {
                return Ok(FungibleAssetKind::Nep245 {
                    account_id: AccountId::try_from(contract_id.to_string())?,
                    token_id: token_id.to_string(),
                });
            }
        }
        Ok(FungibleAssetKind::Nep141(AccountId::try_from(s.to_string())?))
    }
}
