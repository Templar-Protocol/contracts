use crate::{CliError, CliResult};
use serde_json::Value;
use std::{collections::HashMap, fmt};
use templar_common::interest_rate_strategy::InterestRateStrategy;

#[derive(Clone, Debug)]
pub struct StrategyDefaults {
    pub kind: StrategyKind,
    values: HashMap<String, String>,
}

impl StrategyDefaults {
    /// # Errors
    pub fn from_strategy(strategy: &InterestRateStrategy) -> CliResult<Self> {
        let value = serde_json::to_value(strategy)?;
        if let Value::Object(mut map) = value {
            if let Some(linear) = map.remove("Linear") {
                return Ok(Self {
                    kind: StrategyKind::Linear,
                    values: extract_params(&linear)?,
                });
            }
            if let Some(piecewise) = map.remove("Piecewise") {
                return Ok(Self {
                    kind: StrategyKind::Piecewise,
                    values: extract_params(&piecewise)?,
                });
            }
            if let Some(exp) = map.remove("Exponential2") {
                return Ok(Self {
                    kind: StrategyKind::Exponential2,
                    values: extract_params(&exp)?,
                });
            }
        }

        Err(CliError::InvalidInput(
            "Unsupported interest rate strategy format".into(),
        ))
    }

    pub fn get(&self, key: &str, fallback: &str) -> String {
        self.values
            .get(key)
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| fallback.to_string())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum StrategyKind {
    Linear,
    Piecewise,
    Exponential2,
}

impl StrategyKind {
    pub const ALL: [Self; 3] = [Self::Linear, Self::Piecewise, Self::Exponential2];

    pub fn as_index(self) -> usize {
        match self {
            StrategyKind::Linear => 0,
            StrategyKind::Piecewise => 1,
            StrategyKind::Exponential2 => 2,
        }
    }
}

impl fmt::Display for StrategyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StrategyKind::Linear => "Linear",
            StrategyKind::Piecewise => "Piecewise",
            StrategyKind::Exponential2 => "Exponential2",
        })
    }
}

fn extract_params(value: &Value) -> CliResult<HashMap<String, String>> {
    let map = value.as_object().ok_or_else(|| {
        CliError::InvalidInput("Invalid interest rate strategy parameters".into())
    })?;

    Ok(map
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                match v {
                    Value::String(s) => s.clone(),
                    _ => v.to_string(),
                },
            )
        })
        .collect())
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EditSection {
    BasicConfiguration,
    OracleSettings,
    RiskParameters,
    InterestRateStrategy,
    Ranges,
    Fees,
    YieldDistribution,
}

impl EditSection {
    pub const ALL: [Self; 7] = [
        Self::BasicConfiguration,
        Self::OracleSettings,
        Self::RiskParameters,
        Self::InterestRateStrategy,
        Self::Ranges,
        Self::Fees,
        Self::YieldDistribution,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::BasicConfiguration => "Basic configuration",
            Self::OracleSettings => "Oracle settings",
            Self::RiskParameters => "Risk parameters",
            Self::InterestRateStrategy => "Interest rate strategy",
            Self::Ranges => "Ranges",
            Self::Fees => "Fees",
            Self::YieldDistribution => "Yield distribution",
        }
    }
}

impl fmt::Display for EditSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}
