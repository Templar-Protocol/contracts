use clap::{Args, ValueEnum};
use near_account_id::AccountId;
use templar_gateway_methods_spec::storage as spec;
use templar_gateway_types::NearToken;

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
    pub fn try_into_spec(self) -> anyhow::Result<spec::EnsureDeposit> {
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
