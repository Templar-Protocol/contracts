//! Log formatting utilities for human-readable output.
//!
//! Provides functions to format token amounts and other liquidation data
//! for clear, concise logging using actual asset IDs and decimals from
//! market configuration.

/// Derive a short human-readable ticker from a full asset ID.
///
/// Recognizes common patterns:
/// - `nep141:btc.omft.near` → `BTC`
/// - `nep141:eth-0xa0b8...omft.near` (ERC-20 USDC) → `USDC`
/// - `nep141:eth-0x2260...omft.near` (ERC-20 WBTC) → `WBTC`
/// - `nep141:wrap.near` → `wNEAR`
/// - `nep141:meta-pool.near` → `stNEAR`
/// - `17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1` → `USDC`
/// - Stellar OMNI tokens via known suffix fragments
/// - Falls back to the raw ID (truncated) if unknown.
pub fn short_asset_name(asset_id: &str) -> String {
    // Known full asset IDs
    static KNOWN: &[(&str, &str)] = &[
        // NEP-141 OMNI bridge tokens
        ("nep141:btc.omft.near", "BTC"),
        ("nep141:zec.omft.near", "ZEC"),
        (
            "nep141:eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near",
            "USDC",
        ),
        (
            "nep141:eth-0x2260fac5e5542a773aa44fbcfedf7c193bc2c599.omft.near",
            "WBTC",
        ),
        (
            "nep141:sol-5ce3bf3a31af18be40ba30f721101b4341690186.omft.near",
            "USDC",
        ),
        // Native NEAR tokens
        ("nep141:wrap.near", "wNEAR"),
        ("nep141:meta-pool.near", "stNEAR"),
        ("nep141:linear-protocol.near", "LiNEAR"),
        (
            "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
            "USDC",
        ),
    ];

    // Stellar OMNI tokens — match by known base58 suffix fragments
    static STELLAR_TOKENS: &[(&str, &str)] = &[
        (
            "111bzQBB65GxAPAVoxqmMcgYo5oS3txhqs1Uh1cgahKQUeTUq1TJu",
            "USDC",
        ),
        (
            "111bzQBB5v7AhLyPMDwS8uJgQV24KaAPXtwyVWu2KXbbfQU6NXRCz",
            "XLM",
        ),
        (
            "111bzQBB62XZkuam1hPr5wsG54FvwhYaPvecKwgZo1ZoKMWEXcE2n",
            "PYUSD",
        ),
        (
            "111bzQBB5uBD3Wrr7pthp8XhJsreEcwTVnmjQ1wpbzkvHLEQf3ygS",
            "CETES",
        ),
        (
            "111bzQBB5yT2A5maKJqJQsuNg7BA6VG4S4ZATpqmKYLwYBsfEfh6e",
            "USTRY",
        ),
        (
            "111bzQBB5y5yhcUCbDKaCx4zNjEHQbwLAdvwucCecVzC5Ub7uNKEb",
            "DEJTRSY",
        ),
        (
            "111bzQBB66Lr9d7WU1sDna78SqG5x1ZraFjkpPdiYXjHFRnZJUhuV",
            "DEJAAA",
        ),
        (
            "111bzQBB5xzU1EsXby4ckez2qjWFTBiPoqHzZpPkq1Gr9gB7FQpeZ",
            "SolvBTC",
        ),
    ];

    // Strip outer nep245:intents.near: wrapper if present
    let inner = asset_id
        .strip_prefix("nep245:intents.near:")
        .unwrap_or(asset_id);

    // Check exact matches
    for &(pattern, name) in KNOWN {
        if inner == pattern {
            return name.to_string();
        }
    }

    // Check Stellar OMNI token suffixes
    for &(suffix, name) in STELLAR_TOKENS {
        if inner.contains(suffix) {
            return name.to_string();
        }
    }

    // Generic nep245:{contract}:{token_id} — recurse on the token_id part
    if let Some(rest) = inner.strip_prefix("nep245:") {
        if let Some((_contract, token_id)) = rest.split_once(':') {
            return if token_id.starts_with("nep141:") || token_id.starts_with("nep245:") {
                short_asset_name(token_id)
            } else {
                token_id.to_uppercase()
            };
        }
    }

    // Fallback: try to extract something readable
    // nep141:something.near → SOMETHING
    if let Some(rest) = inner.strip_prefix("nep141:") {
        if let Some(name) = rest.strip_suffix(".near") {
            return name.to_uppercase();
        }
        // nep141:something.omft.near already handled above
        return rest.to_string();
    }

    // Last resort: truncate (char-safe to avoid panics on multi-byte UTF-8)
    if inner.len() > 20 {
        let truncated: String = inner.chars().take(17).collect();
        format!("{truncated}…")
    } else {
        inner.to_string()
    }
}

/// Format token amount with short asset name for notifications.
///
/// Produces compact output like `66.25 XLM` instead of the verbose log format.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
pub fn format_amount_short(amount: u128, decimals: i32, asset_id: &str) -> String {
    let divisor = 10f64.powi(decimals);
    let value = amount as f64 / divisor;
    let name = short_asset_name(asset_id);

    // Use fewer decimal places for readability
    let precision = match decimals {
        0..=2 => 2,
        3..=6 => decimals as usize,
        _ => 6,
    };

    format!("{value:.precision$} {name}")
}

/// Format profit/loss with short asset name for notifications.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
pub fn format_profit_short(
    net_profit: i128,
    liquidation_amount: u128,
    decimals: i32,
    asset_id: &str,
) -> String {
    let divisor = 10f64.powi(decimals);
    let profit_value = net_profit as f64 / divisor;
    let profit_pct = if liquidation_amount > 0 {
        (net_profit as f64 / liquidation_amount as f64) * 100.0
    } else {
        0.0
    };
    let name = short_asset_name(asset_id);

    let precision = match decimals {
        0..=2 => 2,
        3..=6 => decimals as usize,
        _ => 6,
    };

    format!("{profit_value:+.precision$} {name} ({profit_pct:+.1}%)")
}

/// Format token amount with asset ID.
///
/// The decimals parameter MUST come from the market's price oracle configuration,
/// never from hardcoded mappings.
///
/// # Examples
///
/// ```ignore
/// format_amount(12_000_000, 6, "nep141:usdc.near")
/// // "12.000000 [nep141:usdc.near] (12000000 raw)"
///
/// format_amount(14_624, 8, "nep141:btc.omft.near")
/// // "0.00014624 [nep141:btc.omft.near] (14624 raw)"
///
/// format_amount(18477275190, 7, "nep245:v2_1.omni.hot.tg:1100_111bzQBB65GxAPAVoxqmMcgYo5oS3txhqs1Uh1cgahKQUeTUq1TJu")
/// // "1847.7275190 [nep245:v2_1.omni.hot.tg:1100_111bzQBB65GxAPAVoxqmMcgYo5oS3txhqs1Uh1cgahKQUeTUq1TJu] (18477275190 raw)"
/// ```
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
pub fn format_amount(amount: u128, decimals: i32, asset_id: &str) -> String {
    let divisor = 10f64.powi(decimals);
    let value = amount as f64 / divisor;

    // Determine precision based on decimals to show meaningful digits
    // decimals is always non-negative in practice (from oracle config)
    let precision = match decimals {
        0..=2 => 2,
        3..=10 => decimals as usize,
        _ => 8,
    };

    format!("{value:.precision$} [{asset_id}] ({amount} raw)")
}

/// Format profit/loss with sign and percentage.
///
/// # Examples
///
/// ```ignore
/// format_profit(952_425, 12_000_000, 6, "nep141:usdc.near")
/// // "+0.952425 [nep141:usdc.near] (+7.9%) [+952425 raw]"
///
/// format_profit(-500_000, 12_000_000, 6, "nep141:usdc.near")
/// // "-0.500000 [nep141:usdc.near] (-4.2%) [-500000 raw]"
/// ```
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
pub fn format_profit(
    net_profit: i128,
    liquidation_amount: u128,
    decimals: i32,
    asset_id: &str,
) -> String {
    let divisor = 10f64.powi(decimals);
    let profit_value = net_profit as f64 / divisor;
    let profit_pct = if liquidation_amount > 0 {
        (net_profit as f64 / liquidation_amount as f64) * 100.0
    } else {
        0.0
    };

    // decimals is always non-negative in practice (from oracle config)
    let precision = match decimals {
        0..=2 => 2,
        3..=10 => decimals as usize,
        _ => 8,
    };

    format!("{profit_value:+.precision$} [{asset_id}] ({profit_pct:+.1}%) [{net_profit:+} raw]")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_amount() {
        // 6 decimals
        assert_eq!(
            format_amount(12_000_000, 6, "nep141:usdc.near"),
            "12.000000 [nep141:usdc.near] (12000000 raw)"
        );

        // 8 decimals
        assert_eq!(
            format_amount(14_624, 8, "nep141:btc.omft.near"),
            "0.00014624 [nep141:btc.omft.near] (14624 raw)"
        );

        // 18 decimals
        assert_eq!(
            format_amount(1_500_000_000_000_000_000, 18, "nep141:weth.near"),
            "1.50000000 [nep141:weth.near] (1500000000000000000 raw)"
        );

        // 7 decimals (Stellar assets)
        assert_eq!(
            format_amount(18_477_275_190, 7, "nep245:v2_1.omni.hot.tg:xlm"),
            "1847.7275190 [nep245:v2_1.omni.hot.tg:xlm] (18477275190 raw)"
        );

        // Zero amount
        assert_eq!(
            format_amount(0, 6, "nep141:usdc.near"),
            "0.000000 [nep141:usdc.near] (0 raw)"
        );
    }

    #[test]
    fn test_format_profit() {
        // Positive profit
        assert_eq!(
            format_profit(952_425, 12_000_000, 6, "nep141:usdc.near"),
            "+0.952425 [nep141:usdc.near] (+7.9%) [+952425 raw]"
        );

        // Negative profit
        assert_eq!(
            format_profit(-500_000, 12_000_000, 6, "nep141:usdc.near"),
            "-0.500000 [nep141:usdc.near] (-4.2%) [-500000 raw]"
        );

        // Zero profit
        assert_eq!(
            format_profit(0, 12_000_000, 6, "nep141:usdc.near"),
            "+0.000000 [nep141:usdc.near] (+0.0%) [+0 raw]"
        );

        // 7 decimals
        assert_eq!(
            format_profit(1_000_000, 10_000_000, 7, "nep245:v2_1.omni.hot.tg:xlm"),
            "+0.1000000 [nep245:v2_1.omni.hot.tg:xlm] (+10.0%) [+1000000 raw]"
        );
    }

    #[test]
    fn test_format_iteration() {
        assert_eq!(format_iteration(1, 3), "1/3");
        assert_eq!(format_iteration(2, 3), "2/3");
        assert_eq!(format_iteration(3, 3), "3/3 (final)");
    }

    #[test]
    fn test_short_asset_name_known() {
        assert_eq!(short_asset_name("nep141:btc.omft.near"), "BTC");
        assert_eq!(short_asset_name("nep141:wrap.near"), "wNEAR");
        assert_eq!(
            short_asset_name("17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1"),
            "USDC"
        );
    }

    #[test]
    fn test_short_asset_name_nep245_intents_wrapper() {
        // nep245:intents.near: wrapper should be stripped
        assert_eq!(
            short_asset_name("nep245:intents.near:nep141:btc.omft.near"),
            "BTC"
        );
    }

    #[test]
    fn test_short_asset_name_stellar_suffix() {
        assert_eq!(
            short_asset_name("nep245:v2_1.omni.hot.tg:1100_111bzQBB65GxAPAVoxqmMcgYo5oS3txhqs1Uh1cgahKQUeTUq1TJu"),
            "USDC"
        );
        assert_eq!(
            short_asset_name("nep245:v2_1.omni.hot.tg:1100_111bzQBB5v7AhLyPMDwS8uJgQV24KaAPXtwyVWu2KXbbfQU6NXRCz"),
            "XLM"
        );
    }

    #[test]
    fn test_short_asset_name_generic_nep245() {
        // Generic nep245:{contract}:{token_id} should extract token_id
        assert_eq!(short_asset_name("nep245:v2_1.omni.hot.tg:xlm"), "XLM");
        assert_eq!(short_asset_name("nep245:some.contract:usdc"), "USDC");
    }

    #[test]
    fn test_short_asset_name_nep141_near_fallback() {
        assert_eq!(short_asset_name("nep141:mytoken.near"), "MYTOKEN");
    }

    #[test]
    fn test_short_asset_name_truncation() {
        let long_id = "abcdefghijklmnopqrstuvwxyz12345";
        let result = short_asset_name(long_id);
        assert_eq!(result, "abcdefghijklmnopq…");
    }

    #[test]
    fn test_format_amount_short() {
        assert_eq!(
            format_amount_short(12_000_000, 6, "nep141:btc.omft.near"),
            "12.000000 BTC"
        );
        assert_eq!(
            format_amount_short(1_500_000, 6, "nep141:wrap.near"),
            "1.500000 wNEAR"
        );
    }

    #[test]
    fn test_format_profit_short() {
        assert_eq!(
            format_profit_short(952_425, 12_000_000, 6, "nep141:btc.omft.near"),
            "+0.952425 BTC (+7.9%)"
        );
        assert_eq!(
            format_profit_short(-500_000, 12_000_000, 6, "nep141:wrap.near"),
            "-0.500000 wNEAR (-4.2%)"
        );
    }

    #[test]
    fn test_short_asset_name_recursive_nep245_with_nep141() {
        // nep245 wrapping nep141 should recurse and resolve
        assert_eq!(
            short_asset_name("nep245:some.contract:nep141:btc.omft.near"),
            "BTC"
        );
    }

    #[test]
    fn test_format_profit_short_zero_liquidation_amount() {
        // Division by zero guard — should produce 0.0%
        assert_eq!(
            format_profit_short(0, 0, 6, "nep141:usdc.near"),
            "+0.000000 USDC (+0.0%)"
        );
    }
}
