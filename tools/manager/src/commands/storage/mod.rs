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
    /// Read the contract's storage balance bounds.
    GetBalanceBounds(GetBalanceBounds),
    /// Read an account's storage balance.
    GetBalanceOf(GetBalanceOf),
    /// Deposit NEAR for storage on the contract.
    Deposit(StorageDeposit),
    /// Unregister storage on the contract.
    Unregister(Unregister),
    /// Ensure an account's storage balance meets a target.
    EnsureDeposit(EnsureDeposit),
}
