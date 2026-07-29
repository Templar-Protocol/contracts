//! Runtime dependencies and manifest persistence boundaries for command handlers.

use std::{fs, io};

use anyhow::Context;
use tracing::debug;

use crate::{
    cli::{Cli, Commands, DeployCommand},
    manifest::Manifest,
    stellar::{CommandExecutor, Stellar},
};

pub(super) struct CommandContext<'a, E: CommandExecutor> {
    cli: &'a Cli,
    executor: &'a E,
    stellar: Stellar<'a, E>,
}

impl<'a, E: CommandExecutor> CommandContext<'a, E> {
    pub(super) fn new(cli: &'a Cli, executor: &'a E) -> Self {
        Self {
            cli,
            executor,
            stellar: Stellar::new(cli, executor),
        }
    }

    pub(super) const fn cli(&self) -> &'a Cli {
        self.cli
    }

    pub(super) const fn executor(&self) -> &'a E {
        self.executor
    }

    pub(super) const fn stellar(&self) -> &Stellar<'a, E> {
        &self.stellar
    }

    pub(super) fn load_manifest(&self) -> anyhow::Result<Manifest> {
        let cli = self.cli;
        if !cli.fresh_state {
            return Manifest::load_or_new(&cli.state, &cli.network, cli.rpc_url.clone());
        }
        if matches!(
            &cli.command,
            Commands::Deploy(args) if matches!(&args.command, DeployCommand::Stack(_))
        ) && !cli.dry_run
        {
            return Manifest::create_new(&cli.state, &cli.network, cli.rpc_url.clone());
        }
        match fs::symlink_metadata(&cli.state) {
            Ok(_) => anyhow::bail!(
                "fresh deployment requires an unused --state path; {} already exists",
                cli.state.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("inspect deployment manifest path {}", cli.state.display())
                });
            }
        }
        Ok(Manifest::new(&cli.network, cli.rpc_url.clone()))
    }

    pub(super) fn checkpoint(&self, manifest: &Manifest) -> anyhow::Result<()> {
        checkpoint_manifest(self.cli, manifest)
    }
}

pub(super) fn checkpoint_manifest(cli: &Cli, manifest: &Manifest) -> anyhow::Result<()> {
    if cli.dry_run {
        debug!(
            manifest = %cli.state.display(),
            "skipping manifest checkpoint during dry run"
        );
        return Ok(());
    }
    debug!(
        manifest = %cli.state.display(),
        contracts = manifest.contracts.len(),
        transactions = manifest.transactions.len(),
        "checkpointing deployment manifest"
    );
    manifest.save(&cli.state)
}
