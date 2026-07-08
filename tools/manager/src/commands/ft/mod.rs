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
    /// Read an account's token balance.
    GetBalanceOf(GetBalanceOf),
    /// Transfer tokens to another account.
    Transfer(Transfer),
    /// Transfer tokens to a contract and invoke its `ft_on_transfer`.
    TransferCall(TransferCall),
}
