use console::Term;
use indicatif::ProgressBar;
use templar_common::interest_rate_strategy::InterestRateStrategy;

use super::types::INTERACTIVE_STEPS;
use crate::ConfigBuilder;

/// Returns a human-readable label for an interest rate strategy.
pub fn strategy_label(strategy: &InterestRateStrategy) -> &'static str {
    match strategy {
        InterestRateStrategy::Linear(_) => "Linear",
        InterestRateStrategy::Piecewise(_) => "Piecewise",
        InterestRateStrategy::Exponential2(_) => "Exponential2",
    }
}

/// Prints the current step overview to the terminal.
pub fn print_step_overview(
    progress: &ProgressBar,
    builder: &ConfigBuilder,
    step_index: u64,
    step_label: &str,
) {
    let term = Term::stdout();
    let _ = term.clear_screen();

    let total = progress.length().unwrap_or(INTERACTIVE_STEPS);
    let position = step_index + 1;

    let _ = term.write_line("Current config");
    for line in builder.overview_lines() {
        let _ = term.write_line(&format!("  • {line}"));
    }
    let _ = term.write_line("");

    progress.set_position(step_index);
    progress.set_message(step_label.to_string());
    progress.tick();

    let _ = term.write_line(&format!("[{position}/{total}] {step_label}"));
    let _ = term.write_line("");
}
