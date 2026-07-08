use clap::Args;
use near_account_id::AccountId;

#[derive(Args, Debug)]
pub struct ClearDeployments {
    /// Registry whose deployments to clear.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Recovered assets and balances are sent here (defaults to the registry).
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: Option<AccountId>,
    /// Continue past a market that fails to remove instead of stopping.
    #[arg(long)]
    force: bool,
}

impl ClearDeployments {
    pub fn registry_id(&self) -> &AccountId {
        &self.registry_id
    }

    /// Beneficiary for recovered funds, defaulting to the registry account.
    pub fn beneficiary_id(&self) -> AccountId {
        self.beneficiary_id
            .clone()
            .unwrap_or_else(|| self.registry_id.clone())
    }

    pub fn force(&self) -> bool {
        self.force
    }
}
