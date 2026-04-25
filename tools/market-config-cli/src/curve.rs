use crate::CliResult;
use clap::ValueEnum;
use templar_common::{interest_rate_strategy::InterestRateStrategy, Decimal};

#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
pub enum ModelArg {
    Piecewise,
    Linear,
    Exponential,
}

impl ModelArg {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Piecewise => "piecewise",
            Self::Linear => "linear",
            Self::Exponential => "exponential",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CurveInput {
    pub starting_rate: Option<Decimal>,
    pub optimal_rate: Option<Decimal>,
    pub optimal_usage: Option<Decimal>,
    pub max_rate: Option<Decimal>,
    pub display_points: usize,
    pub model: Option<ModelArg>,
    pub eccentricity: Option<Decimal>,
}

impl CurveInput {
    pub const fn any_flag_provided(&self) -> bool {
        self.starting_rate.is_some()
            || self.optimal_rate.is_some()
            || self.optimal_usage.is_some()
            || self.max_rate.is_some()
            || self.model.is_some()
            || self.eccentricity.is_some()
    }
}

/// # Errors
pub fn strategy_from_name(name: &str) -> CliResult<InterestRateStrategy> {
    match name {
        "linear" => InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO)
            .ok_or_else(|| crate::CliError::InvalidInput("Invalid linear model seed".into())),
        "exponential" => {
            InterestRateStrategy::exponential2(Decimal::ZERO, Decimal::ZERO, Decimal::from(2u32))
                .ok_or_else(|| {
                    crate::CliError::InvalidInput("Invalid exponential model seed".into())
                })
        }
        "piecewise" => InterestRateStrategy::piecewise(
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
        )
        .ok_or_else(|| crate::CliError::InvalidInput("Invalid piecewise model seed".into())),
        other => Err(crate::CliError::InvalidInput(format!(
            "Unknown model '{other}'"
        ))),
    }
}
