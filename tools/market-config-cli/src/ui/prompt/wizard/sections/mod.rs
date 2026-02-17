pub mod basic;
pub mod fees;
pub mod interest_rate;
pub mod oracle;
pub mod ranges;
pub mod risk;
pub mod yield_weights;

pub use basic::{edit_basic_config, prompt_basic_config};
pub use fees::{edit_fees, prompt_fees};
pub use interest_rate::{edit_interest_rate_strategy, prompt_interest_rate_strategy};
pub use oracle::{edit_oracle_config, prompt_oracle_config};
pub use ranges::{edit_ranges, prompt_ranges};
pub use risk::{edit_risk_parameters, prompt_risk_parameters};
pub use yield_weights::{edit_yield_weights, prompt_yield_weights};
