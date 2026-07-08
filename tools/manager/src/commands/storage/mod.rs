mod deposit;
mod ensure_deposit;
mod get_balance_bounds;
mod get_balance_of;
mod unregister;

pub use deposit::StorageDeposit;
pub use ensure_deposit::EnsureDeposit;
pub use get_balance_bounds::GetBalanceBounds;
pub use get_balance_of::GetBalanceOf;
pub use unregister::Unregister;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum StorageNs {
    GetBalanceBounds(GetBalanceBounds),
    GetBalanceOf(GetBalanceOf),
    Deposit(StorageDeposit),
    Unregister(Unregister),
    EnsureDeposit(EnsureDeposit),
}
