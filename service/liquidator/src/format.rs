//! Log formatting utilities for human-readable output.
//!
//! Provides functions to format token amounts and other liquidation data
//! for clear, concise logging using actual asset IDs and decimals from
//! market configuration.

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
}
