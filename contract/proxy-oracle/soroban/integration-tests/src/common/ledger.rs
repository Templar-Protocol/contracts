//! Ledger advancement helpers. One Stellar ledger close ≈ 5 seconds, so
//! advancing time and sequence in lockstep keeps tests faithful.

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::Env;

pub const SECS_PER_LEDGER: u64 = 5;

/// Move forward by `ledgers` ledger closes (sequence + timestamp both bumped).
pub fn advance_ledgers(env: &Env, ledgers: u32) {
    env.ledger().with_mut(|li| {
        li.sequence_number = li.sequence_number.saturating_add(ledgers);
        li.timestamp = li
            .timestamp
            .saturating_add(u64::from(ledgers) * SECS_PER_LEDGER);
    });
}

/// Move forward by `secs` wall-clock seconds. The sequence advances by the
/// number of whole ledger closes (`secs / SECS_PER_LEDGER`), so sub-5s steps
/// stay within the current ledger and a zero step is a no-op — keeping the
/// advertised time/ledger ratio honest.
pub fn advance_secs(env: &Env, secs: u64) {
    if secs == 0 {
        return;
    }
    let ledgers = u32::try_from(secs / SECS_PER_LEDGER).unwrap_or(u32::MAX);
    env.ledger().with_mut(|li| {
        li.sequence_number = li.sequence_number.saturating_add(ledgers);
        li.timestamp = li.timestamp.saturating_add(secs);
    });
}
