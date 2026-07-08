mod get_balance_of;
mod transfer;
mod transfer_call;

pub use get_balance_of::GetBalanceOf;
pub use transfer::Transfer;
pub use transfer_call::TransferCall;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum FtNs {
    GetBalanceOf(GetBalanceOf),
    Transfer(Transfer),
    TransferCall(TransferCall),
}
