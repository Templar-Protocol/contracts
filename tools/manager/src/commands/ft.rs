use clap::{Args, Subcommand};
use near_account_id::AccountId;
use templar_gateway_methods_spec::ft as spec;
use templar_primitives::SU128;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum FtNs {
    GetBalanceOf(GetBalanceOf),
    Transfer(Transfer),
    TransferCall(TransferCall),
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
pub struct Transfer {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    receiver_id: AccountId,
    #[arg(long, value_name = "AMOUNT")]
    amount: u128,
    #[arg(long, value_name = "TEXT")]
    memo: Option<String>,
}

impl Transfer {
    pub fn parse(self) -> spec::Transfer {
        spec::Transfer {
            contract_id: self.contract_id,
            receiver_id: self.receiver_id,
            amount: SU128::from(self.amount),
            memo: self.memo,
        }
    }
}

#[derive(Args, Debug)]
pub struct TransferCall {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    receiver_id: AccountId,
    #[arg(long, value_name = "AMOUNT")]
    amount: u128,
    #[arg(long, value_name = "TEXT")]
    msg: String,
    #[arg(long, value_name = "TEXT")]
    memo: Option<String>,
}

impl TransferCall {
    pub fn parse(self) -> spec::TransferCall {
        spec::TransferCall {
            contract_id: self.contract_id,
            receiver_id: self.receiver_id,
            amount: SU128::from(self.amount),
            msg: self.msg,
            memo: self.memo,
        }
    }
}
