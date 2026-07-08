use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;
use near_api::PublicKey as CliPublicKey;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::{primitive::PublicKey, Base64Bytes, NearToken};

use crate::context::CliContext;

/// Deploy an already-registered contract version to a new account, granting full
/// access keys (the signer's by default) so the operator retains control.
#[derive(Args, Debug)]
pub struct Deploy {
    /// Registry that holds the version to deploy.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Sub-account name to create under the registry.
    #[arg(long, value_name = "NAME")]
    name: String,
    /// Version key of the contract in the registry.
    #[arg(long, value_name = "KEY")]
    version_key: String,
    /// JSON file with the contract's init args (defaults to `null`).
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<std::path::PathBuf>,
    /// Additional full access keys for the new account. The signer's key is
    /// added by default (unless `--no-signer-full-access-key`).
    #[arg(long = "with-full-access-key", value_name = "PUBLIC_KEY")]
    with_full_access_key: Vec<CliPublicKey>,
    /// Do not grant the signer's public key a full access key on the new account.
    #[arg(long)]
    no_signer_full_access_key: bool,
    /// Deposit funding the new account's storage and balance.
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl Deploy {
    pub fn try_into_spec(self, ctx: &CliContext) -> anyhow::Result<spec::Deploy> {
        let full_access_keys = resolve_full_access_keys(
            ctx.signer_public_key()?,
            self.no_signer_full_access_key,
            &self.with_full_access_key,
        );

        let init_bytes = match self.init_args_file {
            Some(path) => std::fs::read(&path)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("read init args from {}", path.display()))?,
            None => b"null".to_vec(),
        };

        Ok(spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_bytes),
            full_access_keys: Some(full_access_keys),
            deposit: self.deposit,
        })
    }
}

/// Resolve the full access keys granted to an account deployed from a registry:
/// the signer's key by default (so the operator retains control), unless
/// suppressed, plus any explicitly-provided keys, de-duplicated.
fn resolve_full_access_keys(
    signer_public_key: PublicKey,
    no_signer: bool,
    extra: &[CliPublicKey],
) -> Vec<PublicKey> {
    let mut keys = Vec::with_capacity(extra.len() + 1);
    if !no_signer {
        keys.push(signer_public_key);
    }
    for key in extra {
        let key = PublicKey::from(*key);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use crate::commands::registry::RegistryNs;
    use clap::Parser;

    const KEY_A: &str = "ed25519:5ZyGnGUdSp1pj7BbjTyWBvUyC2nh4RdkZyTphUYG4c4v";
    const KEY_B: &str = "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w";

    fn pubkey(s: &str) -> CliPublicKey {
        s.parse().expect("valid ed25519 public key")
    }

    fn primitive(s: &str) -> PublicKey {
        PublicKey::from(pubkey(s))
    }

    #[test]
    fn fak_includes_signer_key_by_default() {
        // The signer's key is granted a full access key on every from-registry
        // deploy — the operator must retain control of the new account.
        let keys = resolve_full_access_keys(primitive(KEY_A), false, &[]);
        assert_eq!(keys, vec![primitive(KEY_A)]);
    }

    #[test]
    fn fak_no_signer_flag_drops_signer_key() {
        assert!(resolve_full_access_keys(primitive(KEY_A), true, &[]).is_empty());
        // With extra keys and --no-signer, only the extras are granted.
        let keys = resolve_full_access_keys(primitive(KEY_A), true, &[pubkey(KEY_B)]);
        assert_eq!(keys, vec![primitive(KEY_B)]);
    }

    #[test]
    fn fak_appends_and_dedups_extra_keys() {
        let keys =
            resolve_full_access_keys(primitive(KEY_A), false, &[pubkey(KEY_B), pubkey(KEY_A)]);
        // Signer key first, then the distinct extra; the duplicate is dropped.
        assert_eq!(keys, vec![primitive(KEY_A), primitive(KEY_B)]);
    }

    #[test]
    fn registry_deploy_resolves_signer_and_extra_faks() {
        let cli = Cli::try_parse_from([
            "tmplrmgr",
            "registry",
            "deploy",
            "--registry-id",
            "registry.testnet",
            "--name",
            "market",
            "--version-key",
            "market@1",
            "--with-full-access-key",
            KEY_B,
            "--deposit",
            "6 NEAR",
        ])
        .expect("registry deploy should parse");
        let Command::Registry {
            command: RegistryNs::Deploy(cmd),
        } = cli.command
        else {
            panic!("expected registry deploy");
        };
        // into_spec resolves these against the signer key (covered by the fak_*
        // tests); assert the parsed flags feed resolution to the signer + extra.
        assert!(!cmd.no_signer_full_access_key);
        let keys = resolve_full_access_keys(
            primitive(KEY_A),
            cmd.no_signer_full_access_key,
            &cmd.with_full_access_key,
        );
        assert_eq!(keys, vec![primitive(KEY_A), primitive(KEY_B)]);
    }
}
