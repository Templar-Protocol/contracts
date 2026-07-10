use anyhow::{bail, Context as _};
use templar_common::Nanoseconds;

/// Parse a human-readable duration into [`Nanoseconds`] for use as a clap
/// `value_parser`, so time flags accept units instead of a raw nanosecond count.
///
/// A unit suffix is required — nanoseconds through days, with common aliases
/// (`ns`/`nanos`, `us`/`µs`/`micros`, `ms`/`millis`, `s`/`sec`/`secs`,
/// `m`/`min`/`mins`, `h`/`hr`/`hrs`, `d`/`day`/`days`) — e.g. `10s`, `100ns`,
/// `2hrs`. Bare numbers are rejected so units are always explicit.
pub fn parse_duration(input: &str) -> anyhow::Result<Nanoseconds> {
    let input = input.trim();
    let boundary = input
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(input.len());
    let (value, unit) = input.split_at(boundary);
    let unit = unit.trim();
    if unit.is_empty() {
        bail!("duration `{input}` needs a unit suffix (e.g. ns, ms, s, m, h, d)");
    }
    let value: u64 = value
        .parse()
        .context("duration must start with a whole number")?;

    let per_unit: u64 = match unit {
        "ns" | "nano" | "nanos" | "nanosecond" | "nanoseconds" => 1,
        "us" | "µs" | "micro" | "micros" | "microsecond" | "microseconds" => 1_000,
        "ms" | "milli" | "millis" | "millisecond" | "milliseconds" => 1_000_000,
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000_000_000,
        "m" | "min" | "mins" | "minute" | "minutes" => 60 * 1_000_000_000,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600 * 1_000_000_000,
        "d" | "day" | "days" => 86_400 * 1_000_000_000,
        other => bail!("unknown duration unit `{other}` (use ns, us, ms, s, m, h, or d)"),
    };

    let ns = value
        .checked_mul(per_unit)
        .context("duration overflows u64 nanoseconds")?;
    Ok(Nanoseconds::from_ns(ns))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(parse_duration("100ns").unwrap().as_ns(), 100);
        assert_eq!(parse_duration("5us").unwrap().as_ns(), 5_000);
        assert_eq!(parse_duration("3ms").unwrap().as_ns(), 3_000_000);
        assert_eq!(parse_duration("2s").unwrap().as_ns(), 2_000_000_000);
        assert_eq!(parse_duration("1m").unwrap().as_ns(), 60_000_000_000);
        assert_eq!(parse_duration("1h").unwrap().as_ns(), 3_600_000_000_000);
        assert_eq!(parse_duration("1d").unwrap().as_ns(), 86_400_000_000_000);
        assert_eq!(parse_duration(" 10 s ").unwrap().as_ns(), 10_000_000_000);
        // Longer aliases.
        assert_eq!(parse_duration("5secs").unwrap().as_ns(), 5_000_000_000);
        assert_eq!(parse_duration("2hrs").unwrap().as_ns(), 7_200_000_000_000);
        assert_eq!(parse_duration("100nanos").unwrap().as_ns(), 100);
        assert_eq!(parse_duration("3mins").unwrap().as_ns(), 180_000_000_000);
    }

    #[test]
    fn rejects_bare_unknown_and_overflow() {
        assert!(parse_duration("100").is_err(), "bare numbers need a unit");
        assert!(parse_duration("10years").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("18446744073709551615s").is_err());
    }
}
