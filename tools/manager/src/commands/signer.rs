//! Per-operation signer inputs. Clap selects either execution credentials or a
//! plan-only output format on every write; reads reject signer arguments.
//! Dispatch rejects print mode only for orchestrated writes that cannot produce
//! one transaction without performing execution steps.
//!
//! Credentials resolve to an [`Arc<Signer>`] rather than a [`SecretKey`], so a
//! backend that holds its key outside this process is expressible.
//! `--sign-with` selects the backend; `--print sputnik` remains the key-less path and never
//! reaches credential resolution at all.

use std::{ffi::OsStr, fmt, sync::Arc};

use anyhow::Context as _;
use clap::{builder::TypedValueParser, error::ErrorKind, Args, ValueEnum};
use near_account_id::AccountId;
use near_api::{NetworkConfig, PublicKey as CliPublicKey, SecretKey, Signer};
use templar_gateway_types::{primitive::PublicKey, ManagedAccountId};

/// Placeholder for the secret key in `Debug` output, so `{:?}` on a command that
/// flattens these args never echoes credentials.
const REDACTED: &str = "<redacted>";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum PrintFormat {
    Json,
    Sputnik,
}

/// A backend that holds the signing key *outside* this process.
///
/// The in-process path — `--secret-key` / `$SECRET_KEY` — is deliberately not a
/// variant. Naming it would let `--sign-with secret-key` satisfy clap's
/// "a backend was chosen" condition while supplying no key, so the invocation
/// would parse and only fail later. Absent `--sign-with`, the secret key is
/// required; present, it is not.
///
/// A Ledger backend was tried and removed: near-api's `ledger` feature pulls
/// hidapi, which needs libudev headers, and cargo unifies features across a
/// workspace build — so every crate here and in CI would inherit that system
/// dependency for something nothing needs yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum SigningBackend {
    /// The OS keychain, looked up by account id.
    Keychain,
}

/// The account authorizing a write and either its execution credential or its
/// plan-only output format. Clap enforces the credential-mode split.
///
/// `Debug` is hand-written to redact `secret_key`; do not derive it.
#[derive(Args, Clone)]
pub struct SignerArgs {
    /// Account that signs the transaction, or the DAO account that will execute a plan.
    #[arg(long, env = "SIGNER_ID", value_name = "ACCOUNT_ID")]
    signer_id: AccountId,
    /// Hold the signing key outside this process. Omit to use `--secret-key`.
    #[arg(long, value_enum, value_name = "BACKEND")]
    sign_with: Option<SigningBackend>,
    /// Private key for `--signer-id`, in `ed25519:…` form.
    ///
    /// Required only for the default `secret-key` backend: naming any
    /// `--sign-with` backend lifts the requirement, and `--print` needs no
    /// credential at all.
    #[arg(
        long,
        env = "SECRET_KEY",
        hide_env_values = true,
        value_name = "SECRET_KEY",
        // `--sign-with` names only external backends, so its presence always
        // means some other credential source was chosen.
        required_unless_present_any = ["print", "sign_with"],
        conflicts_with = "print",
        value_parser = SecretKeyParser
    )]
    secret_key: Option<Box<SecretKey>>,
    /// Plan the write without executing it, then print the selected representation.
    #[arg(long, value_enum, value_name = "FORMAT")]
    print: Option<PrintFormat>,
    /// Public key embedded by deploy/create writes. Required in plan-only mode,
    /// and for backends that hold their key outside this process.
    ///
    /// Deliberately not in conflict with `--secret-key`: an ambient `SECRET_KEY`
    /// is extremely common, and a conflict would make the documented
    /// `--sign-with keychain --public-key …` flow fail at parse time until the
    /// operator unset an environment variable the selected backend ignores.
    #[arg(long, value_name = "PUBLIC_KEY")]
    public_key: Option<CliPublicKey>,
}

impl fmt::Debug for SignerArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignerArgs")
            .field("secret_key", &self.secret_key.as_ref().map(|_| REDACTED))
            .field("signer_id", &self.signer_id)
            .field("sign_with", &self.sign_with)
            .field("print", &self.print)
            .field("public_key", &self.public_key)
            .finish()
    }
}

impl SignerArgs {
    /// The signing account.
    pub fn account_id(&self) -> ManagedAccountId {
        ManagedAccountId::from(self.signer_id.clone())
    }

    /// The requested plan-only output format.
    pub const fn print(&self) -> Option<PrintFormat> {
        self.print
    }

    /// The signer's public key, used by from-registry deploys that grant it a
    /// full access key on the new account.
    ///
    /// Only the `secret-key` backend can derive this locally. Plan mode has no
    /// credential, and the keychain backend holds its key outside this process,
    /// so both require `--public-key`.
    pub fn public_key(&self) -> anyhow::Result<PublicKey> {
        // The in-process backend derives from the key it will actually sign
        // with. Returning a supplied `--public-key` here would grant a full
        // access key on the new account to a key the signer does not hold,
        // while the transaction is signed by the secret — so a contradicting
        // value is an error, not an override.
        if self.sign_with.is_none() && self.print.is_none() {
            let derived = PublicKey::from(self.secret()?.public_key());
            if let Some(supplied) = &self.public_key {
                anyhow::ensure!(
                    PublicKey::from(*supplied) == derived,
                    "--public-key names a different key than --secret-key signs with. \
                     The new account would grant full access to a key you do not hold; \
                     drop --public-key to use the signer's own."
                );
            }
            return Ok(derived);
        }

        if let Some(public_key) = &self.public_key {
            return Ok(PublicKey::from(*public_key));
        }
        if self.print.is_some() {
            anyhow::bail!(
                "--public-key is required with --print for writes that embed the signer's key"
            );
        }
        if self.sign_with.is_some() {
            anyhow::bail!(
                "--public-key is required with --sign-with for writes that embed the signer's key"
            );
        }
        Ok(PublicKey::from(self.secret()?.public_key()))
    }

    /// The signing account together with a signer for it.
    ///
    /// Async because the keychain backend discovers the account's keys on chain
    /// before matching them against the OS keystore.
    pub async fn resolve(
        &self,
        network: &NetworkConfig,
    ) -> anyhow::Result<(ManagedAccountId, Arc<Signer>)> {
        if self.print.is_some() {
            anyhow::bail!("--print is not supported for this orchestrated write");
        }

        let signer = match self.sign_with {
            None => Signer::from_secret_key(self.secret()?.clone())
                .context("build a signer from --secret-key")?,
            Some(SigningBackend::Keychain) => {
                Signer::from_keystore_with_search_for_keys(self.signer_id.clone(), network)
                    .await
                    .context("find a usable key for this account in the OS keychain")?
            }
        };

        Ok((self.account_id(), signer))
    }

    /// Both callers reject print mode before reaching here, so this only has to
    /// account for a credential that clap left absent.
    fn secret(&self) -> anyhow::Result<&SecretKey> {
        self.secret_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("missing --secret-key"))
    }
}

/// A secret key with no bound account — for teardown flows (e.g. `registry
/// clear-deployments`) that sign many discovered accounts with one authorized
/// key, so there is no single `--signer-id`.
///
/// `Debug` is hand-written to redact `secret_key`; do not derive it.
#[derive(Args, Clone)]
pub struct SecretKeyArgs {
    /// Private key that signs each discovered account, in `ed25519:…` form.
    #[arg(
        long,
        env = "SECRET_KEY",
        hide_env_values = true,
        value_name = "SECRET_KEY",
        value_parser = SecretKeyParser
    )]
    secret_key: Box<SecretKey>,
}

impl fmt::Debug for SecretKeyArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKeyArgs")
            .field("secret_key", &REDACTED)
            .finish()
    }
}

impl SecretKeyArgs {
    /// The parsed secret key.
    pub fn secret(&self) -> SecretKey {
        self.secret_key.as_ref().clone()
    }
}

/// Parse a secret key at the clap boundary without placing its source text in
/// the validation error.
#[derive(Clone)]
struct SecretKeyParser;

impl TypedValueParser for SecretKeyParser {
    type Value = Box<SecretKey>;

    fn parse_ref(
        &self,
        command: &clap::Command,
        _argument: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        value
            .to_str()
            .and_then(|value| value.parse().ok())
            .map(Box::new)
            .ok_or_else(|| {
                clap::Error::raw(ErrorKind::ValueValidation, "invalid --secret-key")
                    .with_cmd(command)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use templar_gateway_client::{Network, NetworkConfigBuilder};

    const SECRET: &str = "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        signer: SignerArgs,
    }

    /// The `secret-key` backend never dials out, so resolution stays offline;
    /// this only satisfies the signature.
    fn offline_network() -> NetworkConfig {
        NetworkConfigBuilder::new(Network::Testnet).build()
    }

    #[test]
    fn debug_redacts_secret_key() {
        let harness = Harness::try_parse_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "--secret-key",
            SECRET,
        ])
        .expect("signer args should parse");
        let rendered = format!("{:?}", harness.signer);
        assert!(
            !rendered.contains(SECRET),
            "secret leaked in Debug: {rendered}"
        );
        assert!(
            rendered.contains(REDACTED),
            "no redaction marker: {rendered}"
        );
        // The account id stays visible for diagnostics.
        assert!(
            rendered.contains("signer.testnet"),
            "signer id missing: {rendered}"
        );
    }

    #[tokio::test]
    async fn plan_mode_rejects_credential_resolution() {
        let signer = SignerArgs {
            signer_id: "dao.near".parse().expect("valid account"),
            sign_with: None,
            secret_key: None,
            print: Some(PrintFormat::Sputnik),
            public_key: None,
        };

        assert_eq!(
            signer
                .resolve(&offline_network())
                .await
                // `Signer` is not `Debug`, so discard the Ok value before asserting.
                .map(|_| ())
                .expect_err("plan mode has no execution credentials")
                .to_string(),
            "--print is not supported for this orchestrated write"
        );
    }

    #[test]
    fn plan_write_that_embeds_a_key_requires_public_key() {
        let signer = SignerArgs {
            signer_id: "dao.near".parse().expect("valid account"),
            sign_with: None,
            secret_key: None,
            print: Some(PrintFormat::Json),
            public_key: None,
        };

        assert!(signer
            .public_key()
            .expect_err("plan must not invent a public key")
            .to_string()
            .contains("--public-key"));
    }

    #[test]
    fn execution_public_key_is_derived_from_typed_secret() {
        let secret_key: SecretKey = SECRET.parse().expect("valid secret");
        let expected = PublicKey::from(secret_key.public_key());
        let signer = SignerArgs {
            signer_id: "signer.testnet".parse().expect("valid account"),
            sign_with: None,
            secret_key: Some(Box::new(secret_key)),
            print: None,
            public_key: None,
        };

        assert_eq!(signer.public_key().expect("derived public key"), expected);
    }

    /// The point of the flag: an operator can sign without a key in the
    /// environment, so naming a backend must lift the `--secret-key` demand.
    #[test]
    fn external_backend_parses_without_a_secret_key() {
        let harness = Harness::try_parse_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "--sign-with",
            "keychain",
        ])
        .expect("external backend should parse with no credential");

        assert_eq!(harness.signer.sign_with, Some(SigningBackend::Keychain));
    }

    /// A key held in the keychain or on a device cannot be derived in-process,
    /// so writes that embed the signer's key must be told what it is.
    #[test]
    fn external_backend_requires_an_explicit_public_key() {
        let signer = SignerArgs {
            signer_id: "signer.testnet".parse().expect("valid account"),
            sign_with: Some(SigningBackend::Keychain),
            secret_key: None,
            print: None,
            public_key: None,
        };

        assert!(signer
            .public_key()
            .expect_err("a device key cannot be derived locally")
            .to_string()
            .contains("--public-key"));
    }
}
