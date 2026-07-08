use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::ft as spec;
use templar_primitives::SU128;

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
    pub fn into_spec(self) -> spec::TransferCall {
        spec::TransferCall {
            contract_id: self.contract_id,
            receiver_id: self.receiver_id,
            amount: SU128::from(self.amount),
            msg: self.msg,
            memo: self.memo,
        }
    }
}
