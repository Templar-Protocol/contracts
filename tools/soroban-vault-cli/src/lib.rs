#![allow(unknown_lints)]

pub mod artifacts;
pub mod cli;
pub mod commands;
pub mod manifest;
pub mod stellar;
pub mod types;

use clap::Parser;

pub fn run() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    commands::run(&cli, &stellar::RealExecutor)
}
