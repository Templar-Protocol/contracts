use std::path::PathBuf;

use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct Upgrade {
    /// Registry to upgrade; its owner signs, since only the owner can call `upgrade`.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Catalogued registry release to upgrade to. Defaults to the newest.
    #[arg(long, value_name = "VERSION")]
    release: Option<String>,
    /// Migrate args passed to `migrate` (raw bytes); none if omitted.
    #[arg(long, value_name = "PATH")]
    migrate_args_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Upgrade {
    pub fn registry_id(&self) -> &AccountId {
        &self.registry_id
    }

    pub fn release(&self) -> Option<&str> {
        self.release.as_deref()
    }

    /// `None` for an absent or empty file: `migrate` reads no args as "no migration".
    pub fn migrate_args(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(path) = &self.migrate_args_file else {
            return Ok(None);
        };
        let migrate_args = std::fs::read(path)
            .with_context(|| format!("read migrate args from {}", path.display()))?;
        Ok((!migrate_args.is_empty()).then_some(migrate_args))
    }
}
