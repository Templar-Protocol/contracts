use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::owner as spec;

/// Read the pending proposed contract owner.
#[derive(Args, Debug)]
pub struct GetProposed {
    /// Contract account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
}

impl GetProposed {
    pub fn into_spec(self) -> spec::GetProposedOwner {
        spec::GetProposedOwner {
            contract_id: self.contract_id,
        }
    }
}
