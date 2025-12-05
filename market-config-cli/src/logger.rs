use console::style;

/// Print a warning with standardized red styling.
pub fn warn(message: impl std::fmt::Display) {
    println!("{}", style(format!("⚠ {message}")).red());
}

/// Print a success message with standardized green styling.
pub fn success(message: impl std::fmt::Display) {
    println!("{}", style(format!("✓ {message}")).green());
}
