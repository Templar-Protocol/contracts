#[derive(Debug, PartialEq, Eq)]
pub enum MigrationError {
    UnsupportedPriorityMinSources(u32),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPriorityMinSources(value) => {
                write!(
                    f,
                    "legacy priority proxy requires unsupported min_sources={value}"
                )
            }
        }
    }
}

impl std::error::Error for MigrationError {}
