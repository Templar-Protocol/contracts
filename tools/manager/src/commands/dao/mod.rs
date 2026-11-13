//! Sputnik proposals and DAO-controlled MPC transactions.

pub mod mpc;

use std::path::PathBuf;

use clap::{Args, Subcommand};
use near_account_id::AccountId;

use crate::commands::signer::SignerArgs;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum DaoNs {
    /// Propose the FunctionCall actions of a single transaction signed by the DAO.
    Propose(Propose),
    /// Read a Sputnik proposal by its exact returned ID.
    GetProposal(GetProposal),
    /// Request and execute DAO-controlled MPC signatures.
    #[command(subcommand)]
    Mpc(mpc::MpcNs),
}

#[derive(Args, Debug)]
pub struct Propose {
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub dao: AccountId,
    /// An ordinary PlannedTransaction from `--print json`; `-` reads stdin.
    #[arg(long, value_name = "PATH")]
    pub plan: PathBuf,
    #[arg(long, value_name = "TEXT")]
    pub description: String,
    /// The proposer submitting to Sputnik, not the DAO that executes the plan.
    #[command(flatten)]
    pub signer: SignerArgs,
}

#[derive(Args, Debug)]
pub struct GetProposal {
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub dao: AccountId,
    #[arg(long)]
    pub proposal_id: u64,
}
