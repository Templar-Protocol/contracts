//! Log formatting utilities for human-readable output.
//!
//! Provides functions to format token amounts, account IDs, and other
//! liquidation data for clear, concise logging.

/// Format token amount with symbol based on decimals.
///
/// # Examples
///
/// ```ignore
/// format_amount(12_000_000, 6, "USDC") // "12.00 USDC [12000000]"
/// format_amount(14_624, 8, "BTC")      // "0.00014624 BTC [14624]"
/// ```
#[allow(clippy::cast_precision_loss)]
pub fn format_amount(amount: u128, decimals: i32, symbol: &str) -> String {
    let divisor = 10f64.powi(decimals);
    let value = amount as f64 / divisor;

    let formatted = match decimals {
        6 => format!("{value:.2} {symbol}"),  // USDC, USDT: 2 decimals
        8 => format!("{value:.8} {symbol}"),  // BTC: 8 decimals
        18 => format!("{value:.6} {symbol}"), // ETH: 6 decimals
        _ => format!("{value:.4} {symbol}"),  // XLM (7), NEAR (24), default: 4 decimals
    };

    format!("{formatted} [{amount}]")
}

/// Format amount with USD value estimate.
///
/// # Examples
///
/// ```ignore
/// format_amount_with_usd(12_000_000, 6, "USDC", 1.0) // "12.00 USDC [12000000] ($12.00)"
/// format_amount_with_usd(14_624, 8, "BTC", 89_500.0) // "0.00014624 BTC [14624] ($13.09)"
/// ```
#[allow(clippy::cast_precision_loss)]
pub fn format_amount_with_usd(amount: u128, decimals: i32, symbol: &str, usd_price: f64) -> String {
    let divisor = 10f64.powi(decimals);
    let value = amount as f64 / divisor;
    let usd_value = value * usd_price;

    let formatted = match decimals {
        6 => format!("{value:.2} {symbol}"),
        8 => format!("{value:.8} {symbol}"),
        18 => format!("{value:.6} {symbol}"),
        _ => format!("{value:.4} {symbol}"),
    };

    format!("{formatted} [{amount}] (${usd_value:.2})")
}

/// Extract readable symbol from asset string.
///
/// Handles various asset formats and adds 'i' prefix for intent-wrapped assets.
///
/// # Examples
///
/// ```ignore
/// asset_symbol("nep141:usdc.near") // "USDC"
/// asset_symbol("nep245:intents.near:nep141:eth-0xa0b8...") // "iUSDC"
/// asset_symbol("nep141:wrap.near") // "NEAR"
/// ```
pub fn asset_symbol(asset: &str) -> &'static str {
    let is_intent = asset.contains("intents.near");
    let asset_lower = asset.to_lowercase();

    // Check for USDC contract address (Ethereum: 0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48)
    // Also check for known USDC contract hashes
    if asset_lower.contains("usdc")
        || asset.contains("0xa0b86991")
        || asset.contains("17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1")
    {
        if is_intent {
            "iUSDC"
        } else {
            "USDC"
        }
    } else if asset_lower.contains("usdt") {
        if is_intent {
            "iUSDT"
        } else {
            "USDT"
        }
    } else if asset_lower.contains("wbtc") {
        if is_intent {
            "iWBTC"
        } else {
            "WBTC"
        }
    } else if asset_lower.contains("btc") {
        if is_intent {
            "iBTC"
        } else {
            "BTC"
        }
    } else if asset_lower.contains("weth") {
        if is_intent {
            "iETH"
        } else {
            "ETH"
        }
    } else if asset_lower.contains("xlm") {
        if is_intent {
            "iXLM"
        } else {
            "XLM"
        }
    } else if asset_lower.contains("stnear") {
        "stNEAR"
    } else if asset_lower.contains("linear") {
        "LiNEAR"
    } else if asset_lower.contains("meta-pool") {
        "stNEAR"
    } else if asset_lower.contains("wrap.near") || asset_lower.contains("wnear") {
        "NEAR"
    } else {
        "TOKEN"
    }
}

/// Format profit/loss with sign and percentage.
///
/// # Examples
///
/// ```ignore
/// format_profit(952_425, 12_000_000, 6, "USDC")  // "+0.95 USDC [+952425] (+7.9%)"
/// format_profit(-500_000, 12_000_000, 6, "USDC") // "-0.50 USDC [-500000] (-4.2%)"
/// ```
#[allow(clippy::cast_precision_loss)]
pub fn format_profit(
    net_profit: i128,
    liquidation_amount: u128,
    decimals: i32,
    symbol: &str,
) -> String {
    let divisor = 10f64.powi(decimals);
    let profit_value = net_profit as f64 / divisor;
    let profit_pct = if liquidation_amount > 0 {
        (net_profit as f64 / liquidation_amount as f64) * 100.0
    } else {
        0.0
    };

    format!("{profit_value:+.2} {symbol} [{net_profit:+}] ({profit_pct:+.1}%)")
}

/// Format iteration status for loop liquidation.
///
/// # Examples
///
/// ```ignore
/// format_iteration(1, 3) // "1/3"
/// format_iteration(3, 3) // "3/3 (final)"
/// ```
pub fn format_iteration(current: u32, max: u32) -> String {
    if current >= max {
        format!("{current}/{max} (final)")
    } else {
        format!("{current}/{max}")
    }
}

/// Returns the number of decimals for a given asset symbol.
///
/// # Examples
///
/// ```ignore
/// asset_decimals("USDC") // 6
/// asset_decimals("BTC")  // 8
/// asset_decimals("ETH")  // 18
/// ```
pub fn asset_decimals(symbol: &str) -> i32 {
    match symbol {
        "BTC" | "iBTC" | "WBTC" | "iWBTC" => 8,
        "DAI" | "ETH" | "iETH" | "WETH" => 18,
        "XLM" | "iXLM" => 7,
        _ => 6, // USDC, USDT, stablecoins, and default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_amount() {
        assert_eq!(
            format_amount(12_000_000, 6, "USDC"),
            "12.00 USDC [12000000]"
        );
        assert_eq!(format_amount(14_624, 8, "BTC"), "0.00014624 BTC [14624]");
        assert_eq!(
            format_amount(1_500_000_000_000_000_000, 18, "ETH"),
            "1.500000 ETH [1500000000000000000]"
        );
        assert_eq!(format_amount(0, 6, "USDC"), "0.00 USDC [0]");
    }

    #[test]
    fn test_asset_symbol() {
        assert_eq!(asset_symbol("nep141:usdc.near"), "USDC");
        assert_eq!(
            asset_symbol(
                "nep245:intents.near:nep141:eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near"
            ),
            "iUSDC"
        );
        assert_eq!(
            asset_symbol("nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1"),
            "USDC" // This is the USDC contract hash on NEAR
        );
        assert_eq!(asset_symbol("nep141:wrap.near"), "NEAR");
        assert_eq!(asset_symbol("nep141:meta-pool.near"), "stNEAR");
    }

    #[test]
    fn test_format_profit() {
        assert_eq!(
            format_profit(952_425, 12_000_000, 6, "USDC"),
            "+0.95 USDC [+952425] (+7.9%)"
        );
        assert_eq!(
            format_profit(-500_000, 12_000_000, 6, "USDC"),
            "-0.50 USDC [-500000] (-4.2%)"
        );
        assert_eq!(
            format_profit(0, 12_000_000, 6, "USDC"),
            "+0.00 USDC [+0] (+0.0%)"
        );
    }

    #[test]
    fn test_format_iteration() {
        assert_eq!(format_iteration(1, 3), "1/3");
        assert_eq!(format_iteration(2, 3), "2/3");
        assert_eq!(format_iteration(3, 3), "3/3 (final)");
    }

    #[test]
    fn test_format_amount_with_usd() {
        assert_eq!(
            format_amount_with_usd(12_000_000, 6, "USDC", 1.0),
            "12.00 USDC [12000000] ($12.00)"
        );
        assert_eq!(
            format_amount_with_usd(14_624, 8, "BTC", 89_500.0),
            "0.00014624 BTC [14624] ($13.09)"
        );
    }

    #[test]
    fn test_asset_decimals() {
        // 6 decimals (stablecoins and default)
        assert_eq!(asset_decimals("USDC"), 6);
        assert_eq!(asset_decimals("iUSDC"), 6);
        assert_eq!(asset_decimals("USDT"), 6);
        assert_eq!(asset_decimals("TOKEN"), 6);

        // 8 decimals (BTC variants)
        assert_eq!(asset_decimals("BTC"), 8);
        assert_eq!(asset_decimals("iBTC"), 8);
        assert_eq!(asset_decimals("WBTC"), 8);
        assert_eq!(asset_decimals("iWBTC"), 8);

        // 18 decimals (ETH, DAI)
        assert_eq!(asset_decimals("ETH"), 18);
        assert_eq!(asset_decimals("iETH"), 18);
        assert_eq!(asset_decimals("WETH"), 18);
        assert_eq!(asset_decimals("DAI"), 18);

        // 7 decimals (XLM)
        assert_eq!(asset_decimals("XLM"), 7);
        assert_eq!(asset_decimals("iXLM"), 7);
    }
}
