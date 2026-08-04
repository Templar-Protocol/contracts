//! The human-readable side of a command's output.
//!
//! Everything here writes to stderr; stdout stays the machine-readable result
//! channel. Checks are reported as they land rather than accumulated, because
//! the preflight is a sequence of RPCs and an operator watching it wants to
//! know which one is slow.

use std::collections::BTreeSet;
use std::io::{IsTerminal as _, Write as _};

use crate::spec::check::{failures, Check, Status};
use crate::spec::plan::PlanFile;

/// How much of a detail fits on a streamed line, chosen so the whole line —
/// mark, id column, detail — stays inside 120 columns. The digest repeats every
/// non-passing check in full, so nothing truncated here is lost.
const DETAIL_WIDTH: usize = 80;

/// Whether stderr should carry color. `NO_COLOR` counts for any value, per the
/// convention: the variable's presence is the signal.
///
/// Shared with the tracing layer, which writes to the same stream — one command
/// emitting colored logs above an uncolored report reads as two programs.
pub(crate) fn stderr_is_color_capable() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Collects checks, printing each as it arrives, and owns the `--skip-check`
/// rewrite so a streamed verdict is never revised afterwards.
pub(crate) struct Reporter {
    sink: Sink,
    /// Whether each check is shown as it lands. `-q` drops the stream; it never
    /// drops the digest or the plan, which are results rather than progress.
    stream: bool,
    checks: Vec<Check>,
    skip: Vec<String>,
    matched: BTreeSet<String>,
    color: bool,
    digested: bool,
}

/// Stderr, or a buffer a test can read back. An enum rather than a `W: Write`
/// parameter: the generic would reach every `async` check helper to buy only
/// what [`Reporter::captured`] already gives.
enum Sink {
    Stderr,
    #[cfg(test)]
    Captured(Vec<u8>),
}

impl Reporter {
    pub(crate) fn new(skip: &[String], quiet: bool) -> Self {
        Self {
            sink: Sink::Stderr,
            stream: !quiet,
            checks: Vec::new(),
            skip: skip.to_vec(),
            matched: BTreeSet::new(),
            color: !quiet && stderr_is_color_capable(),
            digested: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn capturing(skip: &[String]) -> Self {
        Self {
            sink: Sink::Captured(Vec::new()),
            stream: true,
            checks: Vec::new(),
            skip: skip.to_vec(),
            matched: BTreeSet::new(),
            color: false,
            digested: false,
        }
    }

    #[cfg(test)]
    pub(crate) const fn quieted(mut self) -> Self {
        self.stream = false;
        self
    }

    #[cfg(test)]
    pub(crate) fn captured(&self) -> String {
        match &self.sink {
            Sink::Captured(buffer) => String::from_utf8_lossy(buffer).into_owned(),
            Sink::Stderr => String::new(),
        }
    }

    /// A heading for the batch of work about to run.
    pub(crate) fn phase(&mut self, label: &str) {
        if self.stream {
            self.line(&format!("\n{} {label}", self.paint("→", Ansi::Dim)));
        }
    }

    /// Record one verdict and show it.
    pub(crate) fn record(&mut self, mut check: Check) {
        if let Some(reason) = self.suppression(&check) {
            self.matched.insert(check.id.clone());
            check.status = Status::Skipped { reason };
        }

        if self.stream {
            let (mark, style) = match check.status {
                Status::Passed { .. } => ("ok  ", Ansi::Green),
                Status::Failed { .. } => ("FAIL", Ansi::Red),
                Status::Skipped { .. } => ("skip", Ansi::Yellow),
            };
            let detail = truncate(detail_of(&check.status), DETAIL_WIDTH);
            self.line(&format!(
                "  {} {:<32} {detail}",
                self.paint(mark, style),
                check.id,
            ));
        }
        self.checks.push(check);
    }

    pub(crate) fn extend(&mut self, checks: impl IntoIterator<Item = Check>) {
        for check in checks {
            self.record(check);
        }
    }

    /// The rewrite a `--skip-check` calls for, recording the verdict it
    /// suppresses — a bare "skipped" would hide the failure an operator chose
    /// to override.
    fn suppression(&self, check: &Check) -> Option<String> {
        let id = self.skip.iter().find(|id| *id == &check.id)?;
        let previous = match &check.status {
            Status::Passed { detail } => format!("would have passed: {detail}"),
            Status::Failed { detail } => format!("would have failed: {detail}"),
            Status::Skipped { reason } => format!("was already skipped: {reason}"),
        };
        Some(format!("--skip-check {id} ({previous})"))
    }

    /// A skip that matched nothing is a typo, and a typo must not read as a
    /// successful suppression. Called once per command, after every phase has
    /// run: the funding checks do not exist until the steps are built.
    pub(crate) fn ensure_every_skip_matched(&self) -> anyhow::Result<()> {
        for id in &self.skip {
            anyhow::ensure!(
                self.matched.contains(id),
                "--skip-check `{id}` names no check in this run. Check ids are listed \
                 by `spec check`; a typo here would silently suppress nothing."
            );
        }
        Ok(())
    }

    /// The counts, then every non-passing check in full. Failures lead: burying
    /// one in a wall of green is how it stops being read.
    ///
    /// At most once per run. `apply` summarizes before its confirmation prompt
    /// and the gate summarizes before refusing, and a failing `apply` reaches
    /// both — two summaries of the same checks read as two sets of them.
    pub(crate) fn digest(&mut self) {
        if std::mem::replace(&mut self.digested, true) {
            return;
        }
        let failed = failures(&self.checks);
        let skipped = self
            .checks
            .iter()
            .filter(|check| matches!(check.status, Status::Skipped { .. }))
            .count();
        let total = self.checks.len();

        let headline = format!(
            "\n{total} check(s): {} passed, {skipped} skipped, {failed} FAILED",
            total - skipped - failed,
        );
        self.line(&self.paint(&headline, if failed > 0 { Ansi::Red } else { Ansi::Green }));

        self.section("FAILED", Ansi::Red, |status| {
            matches!(status, Status::Failed { .. })
        });
        self.section("SKIPPED — these prove nothing", Ansi::Yellow, |status| {
            matches!(status, Status::Skipped { .. })
        });
    }

    fn section(&mut self, title: &str, style: Ansi, matches: fn(&Status) -> bool) {
        let listed: Vec<_> = self
            .checks
            .iter()
            .filter(|check| matches(&check.status))
            .map(|check| (check.id.clone(), detail_of(&check.status).to_owned()))
            .collect();
        if listed.is_empty() {
            return;
        }

        self.line(&format!("\n{}", self.paint(title, style)));
        for (id, detail) in listed {
            self.line(&format!("  {id}\n    {detail}"));
        }
    }

    pub(crate) fn into_checks(self) -> Vec<Check> {
        self.checks
    }

    pub(crate) fn checks(&self) -> &[Check] {
        &self.checks
    }

    /// The plan, for a human about to authorize it.
    pub(crate) fn render_plan(&mut self, file: &PlanFile) {
        self.line(&format!(
            "Plan for {}.{} on {}",
            file.spec.name,
            file.spec.registry,
            file.spec
                .network()
                .map_or_else(|_| "?".to_owned(), |network| network.to_string())
        ));
        for (index, step) in file.steps.iter().enumerate() {
            self.line(&format!("\n  [{index}] {}", step.label));
            self.line(&format!("      {} -> {}", step.signer_id, step.receiver_id));
            for call in &step.function_calls {
                self.line(&format!(
                    "      {}  deposit {}  gas {}",
                    call.method_name, call.deposit, call.gas
                ));
                // Reported rather than swallowed: this is the only place an
                // operator sees the keys and the init args, so a display that
                // silently shows neither is worse than one that says why.
                match crate::dispatch::plan::granted_keys(call) {
                    Ok(keys) => {
                        for key in keys {
                            self.line(&format!("      grants full access to: {key}"));
                        }
                    }
                    Err(error) => self.line(&format!("      keys unreadable: {error:#}")),
                }
                match crate::dispatch::plan::decoded_init_args(call) {
                    Ok(Some(init_args)) => self.line(&format!("      init_args: {init_args}")),
                    Ok(None) => {}
                    Err(error) => self.line(&format!("      init_args unreadable: {error:#}")),
                }
            }
        }
    }

    /// Free-text progress, for what is not a check.
    pub(crate) fn note(&mut self, message: &str) {
        self.line(message);
    }

    fn line(&mut self, text: &str) {
        match &mut self.sink {
            #[cfg(test)]
            Sink::Captured(buffer) => {
                let _ = writeln!(buffer, "{text}");
            }
            Sink::Stderr => {
                let stderr = std::io::stderr();
                let mut lock = stderr.lock();
                let _ = writeln!(lock, "{text}");
            }
        }
    }

    fn paint(&self, text: &str, style: Ansi) -> String {
        if self.color {
            format!("\x1b[{}m{text}\x1b[0m", style.code())
        } else {
            text.to_owned()
        }
    }
}

#[derive(Clone, Copy)]
enum Ansi {
    Green,
    Red,
    Yellow,
    Dim,
}

impl Ansi {
    const fn code(self) -> &'static str {
        match self {
            Self::Green => "32",
            Self::Red => "31",
            Self::Yellow => "33",
            Self::Dim => "2",
        }
    }
}

const fn detail_of(status: &Status) -> &String {
    match status {
        Status::Passed { detail } | Status::Failed { detail } => detail,
        Status::Skipped { reason } => reason,
    }
}

/// Cut to fit one line, on a character boundary so a multi-byte detail cannot
/// panic here.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let kept: String = text.chars().take(width - 1).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::Reporter;
    use crate::spec::check::{Check, Status};

    fn checks() -> Vec<Check> {
        vec![
            Check {
                id: "config.validate".to_owned(),
                status: Status::passed("ok"),
            },
            Check {
                id: "reference.price.collateral".to_owned(),
                status: Status::failed("off by 4%"),
            },
            Check {
                id: "reference.price.all".to_owned(),
                status: Status::Skipped {
                    reason: "no reference price source".to_owned(),
                },
            },
        ]
    }

    /// The point of `--skip-check`: one verdict is suppressed, everything else
    /// is untouched.
    #[test]
    fn skipping_leaves_every_other_check_alone() {
        let mut reporter = Reporter::capturing(&["reference.price.collateral".to_owned()]);
        reporter.extend(checks());
        let checks = reporter.into_checks();

        assert!(matches!(checks[0].status, Status::Passed { .. }));
        assert!(matches!(checks[1].status, Status::Skipped { .. }));
    }

    /// A suppressed failure must still say what it was: `Skipped` also means
    /// "not run", so hiding a known failure in it turns a red check green with
    /// nothing left to review.
    #[test]
    fn a_skipped_check_records_the_verdict_it_suppressed() {
        let mut reporter = Reporter::capturing(&["reference.price.collateral".to_owned()]);
        reporter.extend(checks());
        let checks = reporter.into_checks();

        let Status::Skipped { reason } = &checks[1].status else {
            panic!("expected Skipped, got {:?}", checks[1].status);
        };
        assert!(
            reason.contains("would have failed") && reason.contains("off by 4%"),
            "the suppressed verdict must survive in the report: {reason}"
        );
    }

    /// A failing `apply` summarizes before its prompt and again at the gate.
    /// Two summaries of one check set read as two sets of checks.
    #[test]
    fn the_digest_is_printed_at_most_once() {
        let mut reporter = Reporter::capturing(&[]);
        reporter.extend(checks());
        reporter.digest();
        let once = reporter.captured();
        reporter.digest();

        assert_eq!(
            reporter.captured(),
            once,
            "the second digest must be a no-op"
        );
    }

    /// A typo must not read as a successful suppression.
    #[test]
    fn an_unknown_skip_id_is_an_error() {
        let mut reporter = Reporter::capturing(&["config.validte".to_owned()]);
        reporter.extend(checks());

        let error = reporter
            .ensure_every_skip_matched()
            .expect_err("a typo names no check");
        assert!(error.to_string().contains("names no check"), "{error:#}");
    }

    /// Every check is visible as it lands, and the counts distinguish the three
    /// verdicts — a skipped check must never be totalled as a passing one.
    #[test]
    fn the_digest_leads_with_failures_and_counts_skips_apart() {
        let mut reporter = Reporter::capturing(&[]);
        reporter.phase("reference prices");
        reporter.extend(checks());
        reporter.digest();
        let output = reporter.captured();

        assert!(output.contains("→ reference prices"), "{output}");
        assert!(
            output.contains("3 check(s): 1 passed, 1 skipped, 1 FAILED"),
            "{output}"
        );

        let (failed, skipped) = (
            output.find("FAILED\n  reference.price.collateral"),
            output.find("SKIPPED"),
        );
        assert!(
            failed.is_some() && skipped.is_some() && failed < skipped,
            "failures must be listed, in full, before the skips: {output}"
        );
        assert!(output.contains("off by 4%"), "{output}");
    }

    /// The streamed line is cut to fit; the digest is where the whole detail
    /// has to survive.
    #[test]
    fn a_long_detail_is_truncated_in_the_stream_but_not_in_the_digest() {
        let detail = "x".repeat(400);
        let mut reporter = Reporter::capturing(&[]);
        reporter.record(Check {
            id: "registry.version.oracle".to_owned(),
            status: Status::failed(detail.clone()),
        });
        let streamed = reporter.captured();
        reporter.digest();
        let full = reporter.captured();

        assert!(!streamed.contains(&detail), "the stream must not run over");
        assert!(streamed.contains('…'), "{streamed}");
        assert!(
            full.contains(&detail),
            "the digest must keep the whole detail"
        );
    }

    /// `-q` drops the per-check stream and nothing else. `apply` renders the
    /// plan through this reporter and then asks the operator to authorize it, so
    /// a quiet flag that swallowed the render would put granted full-access keys
    /// and init args behind a bare yes/no prompt.
    #[test]
    fn quiet_drops_the_stream_but_keeps_the_summary() {
        let mut reporter = Reporter::capturing(&[]).quieted();
        reporter.phase("assets");
        reporter.extend(checks());
        assert_eq!(reporter.captured(), "", "the stream is what -q silences");

        reporter.digest();
        let output = reporter.captured();
        assert!(output.contains("1 FAILED"), "{output}");
        assert!(output.contains("off by 4%"), "{output}");
        assert_eq!(reporter.checks().len(), 3, "checks are still collected");
    }
}
