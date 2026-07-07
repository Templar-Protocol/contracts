use std::{collections::HashSet, path::PathBuf, process::ExitCode, time::Duration};

use clap::{Parser, ValueEnum};

use crate::{workspace_loader, ArtifactId, ArtifactMetadata};

mod scheduler;
use scheduler::prebuild_all;

const DEFAULT_MAX_JOBS: usize = 4;
const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 30 * 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum PrebuildProfile {
    Test,
    Drift,
}

impl PrebuildProfile {
    const fn reproducible(self) -> bool {
        match self {
            Self::Test => false,
            Self::Drift => true,
        }
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value_os_t = default_workspace_root())]
    workspace_root: PathBuf,

    #[arg(long, env = "PREBUILD_TEST_CONTRACTS_JOBS", default_value_t = default_jobs())]
    jobs: usize,

    #[arg(
        long,
        env = "PREBUILD_TEST_CONTRACTS_TIMEOUT_SECS",
        default_value_t = DEFAULT_BUILD_TIMEOUT_SECS
    )]
    timeout_secs: u64,

    /// Build profile for contract artifacts.
    #[arg(long, value_enum, default_value_t = PrebuildProfile::Drift)]
    profile: PrebuildProfile,

    /// Artifact to build; repeat or separate values with commas.
    #[arg(
        long = "artifact",
        value_name = "ARTIFACT",
        value_enum,
        ignore_case = true,
        value_delimiter = ','
    )]
    artifacts: Vec<ArtifactId>,
}

pub fn main() -> ExitCode {
    let args = Args::parse();
    let jobs = args.jobs.max(1);
    let timeout = Duration::from_secs(args.timeout_secs);
    let reproducible = args.profile.reproducible();
    let artifacts = selected_artifacts(&args.artifacts);

    match prebuild_all(&args.workspace_root, jobs, timeout, reproducible, artifacts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn default_jobs() -> usize {
    let available = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    available.clamp(1, DEFAULT_MAX_JOBS)
}

fn default_workspace_root() -> PathBuf {
    workspace_loader::discover_workspace_root().unwrap_or_else(|| PathBuf::from("."))
}

fn selected_artifacts(selection: &[ArtifactId]) -> Vec<&'static ArtifactMetadata> {
    if selection.is_empty() {
        return ArtifactId::ALL.iter().map(|id| id.metadata()).collect();
    }

    let selected = selection.iter().copied().collect::<HashSet<_>>();
    ArtifactId::ALL
        .iter()
        .filter(|id| selected.contains(id))
        .map(|id| id.metadata())
        .collect()
}

#[cfg(test)]
mod tests;
