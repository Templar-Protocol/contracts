//! `tmplrmgr mpc`: request an MPC signature through a Sputnik DAO proposal,
//! review what a proposal signs, and relay the signed result.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use near_account_id::AccountId;
use near_api::CryptoHash;
use templar_gateway_client::Network;

use crate::commands::signer::SignerArgs;
use crate::mpc::{
    payload::PayloadKind,
    signer_contract::{self, KeyType},
};

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum MpcNs {
    /// Show the key the MPC derives for a DAO controlling an account.
    DeriveKey(DeriveKey),
    /// Add the key the MPC derives for a DAO as a full-access key on the signer.
    InstallKey(InstallKey),
    /// Propose that the DAO have the MPC sign a planned transaction.
    Propose(Propose),
    /// Decode and verify what an MPC signing proposal would sign.
    Show(Show),
    /// Relay an executed proposal's signed delegate action, paying its gas.
    Relay(Relay),
    /// Broadcast an executed proposal's signed transaction.
    Broadcast(Broadcast),
}

#[derive(Args, Clone, Debug)]
pub struct MpcContractArgs {
    /// MPC signer contract. Defaults to the network's (`v1.signer` on mainnet).
    #[arg(long, value_name = "ACCOUNT_ID")]
    mpc_contract: Option<AccountId>,
    /// Curve of the derived key.
    #[arg(long, value_enum, default_value_t = KeyType::Ed25519, value_name = "CURVE")]
    pub key_type: KeyType,
}

impl MpcContractArgs {
    pub fn contract_id(&self, network: Network) -> AccountId {
        self.mpc_contract
            .clone()
            .unwrap_or_else(|| signer_contract::default_contract_id(network))
    }
}

#[derive(Args, Clone, Debug)]
pub struct DerivationArgs {
    /// DAO whose proposals call `sign`; the key is derived for it as the caller.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub dao: AccountId,
    /// Derivation path. Defaults to `<dao>-<controlled account>`.
    #[arg(long, value_name = "PATH")]
    path: Option<String>,
}

impl DerivationArgs {
    pub fn path_for(&self, controlled: &AccountId) -> String {
        self.path
            .clone()
            .unwrap_or_else(|| format!("{}-{controlled}", self.dao))
    }
}

#[derive(Args, Debug)]
pub struct DeriveKey {
    #[command(flatten)]
    pub mpc: MpcContractArgs,
    #[command(flatten)]
    pub derivation: DerivationArgs,
    /// The account the derived key controls (names the default path).
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub account_id: AccountId,
}

#[derive(Args, Debug)]
pub struct InstallKey {
    #[command(flatten)]
    pub mpc: MpcContractArgs,
    #[command(flatten)]
    pub derivation: DerivationArgs,
    /// The account the key is installed on, signing with a key it already holds.
    #[command(flatten)]
    pub signer: SignerArgs,
}

#[derive(Args, Debug)]
pub struct Propose {
    #[command(flatten)]
    pub mpc: MpcContractArgs,
    #[command(flatten)]
    pub derivation: DerivationArgs,
    /// A write's `--print json` output; `-` reads stdin. Its signer is the
    /// controlled account.
    #[arg(long, value_name = "PATH")]
    pub plan: PathBuf,
    #[arg(long, value_enum, default_value_t = PayloadKind::DelegateAction, value_name = "KIND")]
    pub kind: PayloadKind,
    /// Nonce for the derived key. Defaults to its current nonce + 1; set it when
    /// several proposals for the same key are in flight.
    #[arg(long)]
    pub nonce: Option<u64>,
    /// How many blocks past the current head a delegate action stays relayable.
    /// The default is ≈7 days of mainnet blocks, our DAOs' `proposal_period`.
    #[arg(long, default_value_t = 1_000_000, value_name = "BLOCKS")]
    pub valid_for_blocks: u64,
    /// Proposal description shown to voters.
    #[arg(long, default_value = "MPC signature request", value_name = "TEXT")]
    pub description: String,
    /// Keep the payload out of the proposal description; voters see only the hash.
    #[arg(long, requires = "out")]
    pub blind: bool,
    /// Write the payload envelope here as well, for `show`/`relay --payload-file`.
    #[arg(long, value_name = "PATH")]
    pub out: Option<PathBuf>,
    /// Gas the DAO attaches to `sign`. The contract needs 15 and refunds the rest.
    #[arg(long, default_value_t = 30, value_name = "TGAS")]
    pub sign_tgas: u64,
    #[command(flatten)]
    pub signer: SignerArgs,
}

#[derive(Args, Debug)]
pub struct Show {
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub dao: AccountId,
    #[arg(long)]
    pub proposal_id: u64,
    /// Read the payload from this file instead of the proposal description.
    #[arg(long, value_name = "PATH")]
    pub payload_file: Option<PathBuf>,
}

/// An executed proposal and the transaction that executed it.
#[derive(Args, Clone, Debug)]
pub struct ExecutedProposalArgs {
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub dao: AccountId,
    #[arg(long)]
    pub proposal_id: u64,
    /// Read the payload from this file instead of the proposal description.
    #[arg(long, value_name = "PATH")]
    pub payload_file: Option<PathBuf>,
    /// The transaction that executed the proposal (its `act_proposal` vote).
    #[arg(long, value_name = "HASH")]
    pub tx_hash: CryptoHash,
    /// Account that sent that transaction. NEAR looks a transaction up by
    /// hash and sender, so a hash alone cannot be resolved.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub tx_signer: AccountId,
}

#[derive(Args, Debug)]
pub struct Relay {
    #[command(flatten)]
    pub executed: ExecutedProposalArgs,
    /// The account that wraps the delegate action in a transaction and pays its gas.
    #[command(flatten)]
    pub signer: SignerArgs,
}

#[derive(Args, Debug)]
pub struct Broadcast {
    #[command(flatten)]
    pub executed: ExecutedProposalArgs,
    /// Print the signed transaction as base64 borsh (what `near transaction
    /// send-signed-transaction` accepts) instead of sending it.
    #[arg(long)]
    pub print: bool,
}
