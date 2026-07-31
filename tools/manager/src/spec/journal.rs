//! An append-only record of what a deploy actually did, so an interrupted one
//! can be resumed instead of restarted.
//!
//! `script/deploy.sh` is not re-runnable, and its own comment documents the
//! cost: a stale version key aborts at the oracle step and leaves an orphaned
//! governance contract to be deleted by hand. Ordering was the only safety
//! property. This replaces it with a mechanism.
//!
//! ## Keyed on the plan, not the spec
//!
//! Each entry records the digest of the step it completed. Resuming recomputes
//! those digests and refuses if one changed — so a hand-edited plan resumes
//! correctly, and an edit *under a completed step* is named rather than
//! silently re-run or silently skipped. Editing a step that has not run yet is
//! fine, which is the point: the artifact exists to be edited.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use super::plan::PlanFile;

/// One completed step. Only successes are appended: a step that failed did not
/// happen, and recording it would make resume skip work still to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub step: usize,
    /// The digest of the step as executed, for reconciliation on resume.
    pub digest: String,
    pub label: String,
    /// Absent when the step completed without the executor reporting a hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub entries: Vec<Entry>,
}

/// Where a plan's journal lives: beside the plan, named after it.
///
/// Derived rather than configurable, so resuming cannot be defeated by
/// forgetting a flag — the failure mode that matters is re-running a deploy
/// from step one, not writing a file to the wrong place.
pub fn path_for(plan: &Path) -> PathBuf {
    let mut name = plan.file_name().unwrap_or_default().to_os_string();
    name.push(".journal.json");
    plan.with_file_name(name)
}

impl Journal {
    /// Read the journal beside `plan`, or an empty one if this is a first run.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("parse the journal at {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            // Anything else — a permissions problem, a directory in the way — is
            // not "no journal". Treating it as one would restart a deploy that
            // may be half-done.
            Err(error) => Err(anyhow::Error::new(error)
                .context(format!("read the journal at {}", path.display()))),
        }
    }

    /// Append an entry and persist immediately.
    ///
    /// Written after every step rather than at the end: a journal that is only
    /// flushed on completion records nothing about the run that was interrupted,
    /// which is the only run it exists for.
    pub fn append(&mut self, path: &Path, entry: Entry) -> anyhow::Result<()> {
        self.entries.push(entry);
        let rendered = serde_json::to_string_pretty(self).context("render the journal")?;
        std::fs::write(path, format!("{rendered}\n"))
            .with_context(|| format!("write the journal at {}", path.display()))
    }

    /// Which steps of `file` still have to run.
    ///
    /// Refuses rather than guessing when the journal and the plan disagree:
    /// every recorded step must still exist and still hash the same. Silently
    /// re-running a completed step would double-spend a deposit; silently
    /// skipping an edited one would deploy something nobody reviewed.
    pub fn remaining(&self, file: &PlanFile) -> anyhow::Result<Vec<usize>> {
        for entry in &self.entries {
            let step = file.steps.get(entry.step).with_context(|| {
                format!(
                    "the journal records step {} (`{}`) as done, but this plan has \
                     only {} step(s). It is a journal for a different plan.",
                    entry.step,
                    entry.label,
                    file.steps.len()
                )
            })?;

            let current = super::plan::digest(step)?;
            anyhow::ensure!(
                current == entry.digest,
                "step {} (`{}`) already ran, but the plan has changed under it. \
                 Re-running it would repeat work that is already done; skipping \
                 it would apply a step nobody executed. Restore that step, or \
                 start from a fresh plan and journal.",
                entry.step,
                entry.label,
            );
        }

        let done: std::collections::BTreeSet<usize> =
            self.entries.iter().map(|entry| entry.step).collect();
        Ok((0..file.steps.len())
            .filter(|index| !done.contains(index))
            .collect())
    }
}
