use clap::{Args, Subcommand, ValueEnum};
use near_account_id::AccountId;
use templar_gateway_methods_spec::storage as spec;
use templar_gateway_types::NearToken;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum StorageNs {
    GetBalanceBounds(GetBalanceBounds),
    GetBalanceOf(GetBalanceOf),
    Deposit(StorageDeposit),
    Unregister(Unregister),
    EnsureDeposit(EnsureDeposit),
}

#[derive(Args, Debug)]
pub struct GetBalanceBounds {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
}

impl GetBalanceBounds {
    pub fn parse(self) -> spec::GetBalanceBounds {
        spec::GetBalanceBounds {
            contract_id: self.contract_id,
        }
    }
}

#[derive(Args, Debug)]
pub struct GetBalanceOf {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl GetBalanceOf {
    pub fn parse(self) -> spec::GetBalanceOf {
        spec::GetBalanceOf {
            contract_id: self.contract_id,
            account_id: self.account_id,
        }
    }
}

#[derive(Args, Debug)]
pub struct StorageDeposit {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: Option<AccountId>,
    #[arg(long)]
    registration_only: bool,
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl StorageDeposit {
    pub fn parse(self) -> spec::Deposit {
        spec::Deposit {
            contract_id: self.contract_id,
            beneficiary_id: self.beneficiary_id,
            registration_only: self.registration_only,
            deposit: self.deposit,
        }
    }
}

#[derive(Args, Debug)]
pub struct Unregister {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[arg(long)]
    force: bool,
}

impl Unregister {
    pub fn parse(self) -> spec::Unregister {
        spec::Unregister {
            contract_id: self.contract_id,
            force: self.force,
        }
    }
}

#[derive(Clone, Debug, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum EnsureModeArg {
    Registered,
    MinimumTotal,
    MinimumAvailable,
}

#[derive(Args, Debug)]
pub struct EnsureDeposit {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
    #[arg(long, value_enum)]
    mode: EnsureModeArg,
    #[arg(long, value_name = "AMOUNT")]
    amount: Option<NearToken>,
}

impl EnsureDeposit {
    pub fn parse(self) -> anyhow::Result<spec::EnsureDeposit> {
        let mode = match self.mode {
            EnsureModeArg::Registered => {
                if self.amount.is_some() {
                    anyhow::bail!(
                        "--amount is only valid for minimum_total or minimum_available mode"
                    );
                }
                spec::EnsureDepositMode::Registered
            }
            EnsureModeArg::MinimumTotal => {
                spec::EnsureDepositMode::MinimumTotal(self.amount.ok_or_else(|| {
                    anyhow::anyhow!("--amount is required for minimum_total mode")
                })?)
            }
            EnsureModeArg::MinimumAvailable => {
                spec::EnsureDepositMode::MinimumAvailable(self.amount.ok_or_else(|| {
                    anyhow::anyhow!("--amount is required for minimum_available mode")
                })?)
            }
        };

        Ok(spec::EnsureDeposit {
            contract_id: self.contract_id,
            account_id: self.account_id,
            mode,
        })
    }
}
