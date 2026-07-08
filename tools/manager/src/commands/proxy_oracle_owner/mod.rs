mod propose_owner;

pub use propose_owner::ProposeOwner;

use clap::{Args, Subcommand};
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_owner as spec;

#[allow(clippy::enum_variant_names)] // these are the contract's own owner ops
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleOwnerNs {
    GetOwner(OracleIdArgs),
    GetProposedOwner(OracleIdArgs),
    ProposeOwner(ProposeOwner),
    AcceptOwner(OracleIdArgs),
    RenounceOwner(OracleIdArgs),
}

/// Shared argument for the owner reads/writes keyed only by the oracle account.
#[derive(Args, Debug)]
pub struct OracleIdArgs {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
}

impl OracleIdArgs {
    pub fn get_owner(self) -> spec::GetOwner {
        spec::GetOwner {
            oracle_id: self.oracle_id,
        }
    }
    pub fn get_proposed_owner(self) -> spec::GetProposedOwner {
        spec::GetProposedOwner {
            oracle_id: self.oracle_id,
        }
    }
    pub fn accept_owner(self) -> spec::AcceptOwner {
        spec::AcceptOwner {
            oracle_id: self.oracle_id,
        }
    }
    pub fn renounce_owner(self) -> spec::RenounceOwner {
        spec::RenounceOwner {
            oracle_id: self.oracle_id,
        }
    }
}
