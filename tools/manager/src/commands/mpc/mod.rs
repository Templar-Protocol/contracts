//! `tmplrmgr mpc`: request an MPC signature through a Sputnik DAO proposal,
//! review what a proposal signs, and relay the signed result.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use near_account_id::AccountId;
use near_api::CryptoHash;
use templar_gateway_client::Network;

use crate::commands::signer::SignerArgs;
use crate::mpc::{payload::PayloadKind, signer_contract::KeyType};

/// ≈7 days of mainnet blocks: the default `proposal_period` of our DAOs.
pub const DEFAULT_VALID_FOR_BLOCKS: u64 = 1_000_000;
/// Above the contract's 15 Tgas floor; the remainder is refunded.
pub const DEFAULT_SIGN_TGAS: u64 = 30;
pub const DEFAULT_ADD_PROPOSAL_TGAS: u64 = 30;

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
    /// Assemble the signature an executed proposal produced and send the result.
    Relay(Relay),
}

#[derive(Args, Clone, Debug)]
pub struct MpcContractArgs {
    /// MPC signer contract. Defaults to the network's (`v1.signer` on mainnet).
    #[arg(long, value_name = "ACCOUNT_ID")]
    mpc_contract: Option<AccountId>,
    /// Curve of the derived key.
    #[arg(long, value_enum, default_value_t = KeyType::Ed25519, value_name = "CURVE")]
    key_type: KeyType,
}

impl MpcContractArgs {
    pub fn contract_id(&self, network: Network) -> AccountId {
        self.mpc_contract
            .clone()
            .unwrap_or_else(|| network.mpc_contract_id())
    }

    pub const fn key_type(&self) -> KeyType {
        self.key_type
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
    #[arg(long, default_value_t = DEFAULT_VALID_FOR_BLOCKS, value_name = "BLOCKS")]
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
    /// Gas the DAO attaches to `sign`.
    #[arg(long, default_value_t = DEFAULT_SIGN_TGAS, value_name = "TGAS")]
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

#[derive(Args, Debug)]
pub struct Relay {
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
    /// Account that sent that transaction.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub tx_signer: AccountId,
    /// Relayer of a delegate-action payload. A transaction payload is already
    /// signed by the derived key, so only `--print` (which emits the signed
    /// transaction as base64 borsh) reads these.
    #[command(flatten)]
    pub signer: SignerArgs,
}
