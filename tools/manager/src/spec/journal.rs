//! An append-only record of what a deploy actually did, so an interrupted one
//! can be resumed instead of restarted.
//!
//! The shell deploy this replaces was not re-runnable, and its own comment
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

/// What happened to a step.
///
/// `Attempted` is written *before* the transaction is submitted. Without it, a
/// step whose outcome is unknown — a timeout, or an operation that never
/// reached a terminal state — is indistinguishable from one that never ran, and
/// resume would re-send it. For a registry deploy that is not a free retry: the
/// deposit is transferred into the batch, and a failed `create_account` refunds
/// it to the registry, not to the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Attempted,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub step: usize,
    /// Digest of the step's executable content, for reconciliation on resume.
    pub digest: String,
    pub label: String,
    pub outcome: Outcome,
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

/// A step's digest over everything but its label.
pub fn executable_digest(step: &super::plan::PlanStep) -> anyhow::Result<String> {
    super::plan::digest(&(&step.signer_id, &step.receiver_id, &step.function_calls))
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

    /// Record an entry and persist immediately.
    ///
    /// Written around every step rather than at the end: a journal flushed only
    /// on completion records nothing about the run that was interrupted, which
    /// is the only run it exists for.
    ///
    /// Rewrites through a temporary file and renames, because this is a
    /// whole-file format: a truncating write re-risks the entire history on
    /// every step, and a crash inside that window — precisely the scenario this
    /// module exists for — would leave unparseable JSON and no record at all.
    pub fn record(&mut self, path: &Path, entry: Entry) -> anyhow::Result<()> {
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.step == entry.step)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }

        let rendered = serde_json::to_string_pretty(self).context("render the journal")?;
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, format!("{rendered}\n"))
            .with_context(|| format!("write {}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("replace the journal at {}", path.display()))
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

            // Digested without the label: it is manager-only and the executor
            // never reads it, so renaming a completed step would otherwise block
            // a resume over a change that alters nothing executable.
            let current = executable_digest(step)?;
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

        // An attempt whose outcome was never resolved is not progress and not
        // absence. Re-sending it can strand a deposit; skipping it deploys
        // nothing. Only a human can tell which, so the run stops.
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.outcome == Outcome::Attempted)
        {
            anyhow::bail!(
                "step {} (`{}`) was submitted but its outcome was never recorded{}. \
                 It may or may not have run. Check the account on chain, then \
                 either mark it `completed` in the journal or remove the entry \
                 to re-send it — re-sending a deploy that succeeded strands its \
                 deposit in the registry.",
                entry.step,
                entry.label,
                entry
                    .tx_hash
                    .as_ref()
                    .map_or_else(String::new, |hash| format!(" (tx {hash})")),
            );
        }

        let done: std::collections::BTreeSet<usize> =
            self.entries.iter().map(|entry| entry.step).collect();

        // Steps run in order and only completions are recorded, so the done set
        // is always a prefix. A journal that skips one — hand-edited, or
        // partially recovered — would make `apply` step over work that never
        // happened, and every plan-level check would still pass because the
        // *plan* is intact. Only the executed set is wrong.
        anyhow::ensure!(
            done.iter().copied().eq(0..done.len()),
            "this journal records steps {done:?} as done, which is not a prefix \
             of the plan. Steps run in order, so a gap means an entry was \
             removed or invented; resuming would skip a step that never ran.",
        );

        Ok((0..file.steps.len())
            .filter(|index| !done.contains(index))
            .collect())
    }
}
