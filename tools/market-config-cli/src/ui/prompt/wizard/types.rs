use crate::{logger, ui::prompt::error::handle_interrupted, CliError, CliResult};
use templar_common::{oracle::pyth::Price, Decimal};

pub const INTERACTIVE_STEPS: u64 = 7;

#[derive(Clone, Copy)]
pub enum AssetStandard {
    Nep141,
    Nep245,
}

#[derive(Clone)]
pub struct PriceHintContext {
    pub price: Price,
    pub asset_decimals: i32,
}

/// Repeatedly prompts until the user provides a valid input.
/// # Errors
pub fn prompt_until_valid<T, R, P, V>(mut prompt_fn: P, mut validate_fn: V) -> CliResult<R>
where
    P: FnMut() -> Result<T, dialoguer::Error>,
    V: FnMut(T) -> CliResult<R>,
{
    loop {
        match prompt_fn() {
            Ok(value) => match validate_fn(value) {
                Ok(result) => break Ok(result),
                Err(err) => {
                    logger::warn(err);
                    println!("Please try again.\n");
                }
            },
            Err(err) => {
                if handle_interrupted(&err) {
                    return Err(CliError::Interrupted);
                }
                logger::warn(format!("Failed to read input: {err}"));
                println!("Please try again.\n");
            }
        }
    }
}

/// Converts a Pyth Price to a Decimal representation.
pub fn price_decimal(price: &Price) -> Option<Decimal> {
    let raw = price.price.0;
    if raw <= 0 {
        return None;
    }
    let abs = u128::try_from(raw).ok()?;
    let base = Decimal::from(abs);
    let scale = Decimal::from_u32(10).pow(price.expo);
    Some(base * scale)
}
