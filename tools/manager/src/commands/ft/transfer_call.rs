use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::ft as spec;
use templar_primitives::SU128;

#[derive(Args, Debug)]
pub struct TransferCall {
    /// Token contract to transfer from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    /// Contract receiving the tokens.
    #[arg(long, value_name = "ACCOUNT_ID")]
    receiver_id: AccountId,
    /// Amount to transfer (in the token's smallest unit).
    #[arg(long, value_name = "AMOUNT")]
    amount: u128,
    /// Message forwarded to the receiver's `ft_on_transfer`.
    #[arg(long, value_name = "TEXT")]
    msg: String,
    /// Optional memo attached to the transfer.
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
