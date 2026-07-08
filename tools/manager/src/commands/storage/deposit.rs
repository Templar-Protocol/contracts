use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::storage as spec;
use templar_gateway_types::NearToken;

#[derive(Args, Debug)]
pub struct StorageDeposit {
    /// Contract to deposit storage on.
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    /// Account credited with the deposit (defaults to the signer).
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: Option<AccountId>,
    /// Register only, depositing just the minimum required balance.
    #[arg(long)]
    registration_only: bool,
    /// Amount of NEAR to deposit.
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl StorageDeposit {
    pub fn into_spec(self) -> spec::Deposit {
        spec::Deposit {
            contract_id: self.contract_id,
            beneficiary_id: self.beneficiary_id,
            registration_only: self.registration_only,
            deposit: self.deposit,
        }
    }
}
