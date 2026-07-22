//! Signer credentials as structural, per-operation arguments. Flattening these
//! into a write command's args makes the invalid states unrepresentable at the
//! parser: a write with no credentials can't parse, and credentials on a read are
//! an unexpected argument — enforcement lives in clap, not a runtime check.

use std::{ffi::OsStr, fmt};

use clap::{builder::TypedValueParser, error::ErrorKind, Args, ValueEnum};
use near_account_id::AccountId;
use near_api::{PublicKey as CliPublicKey, SecretKey};
use templar_gateway_types::{primitive::PublicKey, ManagedAccountId};

/// Placeholder for the secret key in `Debug` output, so `{:?}` on a command that
/// flattens these args never echoes credentials.
const REDACTED: &str = "<redacted>";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum PrintFormat {
    Json,
    Sputnik,
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
    /// Private key for `--signer-id`, in `ed25519:…` form.
    #[arg(
        long,
        env = "SECRET_KEY",
        hide_env_values = true,
        value_name = "SECRET_KEY",
        required_unless_present = "print",
        conflicts_with = "print",
        value_parser = SecretKeyParser
    )]
    secret_key: Option<Box<SecretKey>>,
    /// Plan the write without executing it, then print the selected representation.
    #[arg(long, value_enum, value_name = "FORMAT")]
    print: Option<PrintFormat>,
    /// Public key embedded by deploy/create writes in plan-only mode.
    #[arg(
        long,
        value_name = "PUBLIC_KEY",
        requires = "print",
        conflicts_with = "secret_key"
    )]
    public_key: Option<CliPublicKey>,
}

impl fmt::Debug for SignerArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignerArgs")
            .field("secret_key", &self.secret_key.as_ref().map(|_| REDACTED))
            .field("signer_id", &self.signer_id)
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
    pub fn public_key(&self) -> anyhow::Result<PublicKey> {
        if let Some(public_key) = &self.public_key {
            return Ok(PublicKey::from(*public_key));
        }
        if self.print.is_some() {
            anyhow::bail!(
                "--public-key is required with --print for writes that embed the signer's key"
            );
        }
        Ok(PublicKey::from(self.secret()?.public_key()))
    }

    /// The signing account together with its secret key.
    pub fn resolve(&self) -> anyhow::Result<(ManagedAccountId, SecretKey)> {
        Ok((self.account_id(), self.secret()?))
    }

    fn secret(&self) -> anyhow::Result<SecretKey> {
        if self.print.is_some() {
            anyhow::bail!("--print is not supported for this orchestrated write");
        }
        self.secret_key
            .as_deref()
            .cloned()
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

    const SECRET: &str = "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";
    const PUBLIC: &str = "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w";

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        signer: SignerArgs,
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

    #[test]
    fn invalid_secret_key_is_rejected_without_echoing_it() {
        let invalid = "ed25519:sensitive-but-invalid";
        let error = Harness::try_parse_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "--secret-key",
            invalid,
        ])
        .err()
        .expect("invalid secret should fail at the clap boundary");
        let rendered = error.to_string();
        assert!(rendered.contains("invalid --secret-key"), "{rendered}");
        assert!(!rendered.contains(invalid), "secret leaked: {rendered}");
    }

    #[test]
    fn plan_public_key_is_explicit_and_never_resolves_credentials() {
        let public_key: CliPublicKey = PUBLIC.parse().expect("valid public key");
        let signer = SignerArgs {
            signer_id: "dao.near".parse().expect("valid account"),
            secret_key: None,
            print: Some(PrintFormat::Sputnik),
            public_key: Some(public_key),
        };

        assert_eq!(
            signer.public_key().expect("explicit plan key"),
            PublicKey::from(public_key)
        );
        assert_eq!(signer.print(), Some(PrintFormat::Sputnik));
        assert_eq!(
            signer
                .resolve()
                .expect_err("plan mode has no execution credentials")
                .to_string(),
            "--print is not supported for this orchestrated write"
        );
    }

    #[test]
    fn plan_write_that_embeds_a_key_requires_public_key() {
        let signer = SignerArgs {
            signer_id: "dao.near".parse().expect("valid account"),
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
            secret_key: Some(Box::new(secret_key)),
            print: None,
            public_key: None,
        };

        assert_eq!(signer.public_key().expect("derived public key"), expected);
    }
}
