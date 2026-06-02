//! Fuzz `DepositMsg` JSON (de)serialization — the real codec the market uses
//! to parse the attacker-controlled `msg` string passed to `ft_transfer_call` /
//! `*_on_transfer` (`common/src/market/mod.rs`). This is a genuine
//! untrusted-input surface (P1: real code, P3: reach the parser).
//!
//! Two oracles:
//! 1. **Implicit (raw parse):** `serde_json::from_str::<DepositMsg>` on
//!    arbitrary UTF-8 must never panic — only `Ok`/`Err`. (The contract calls
//!    this on bytes it does not control.)
//! 2. **Round-trip idempotence:** for a constructed `DepositMsg`,
//!    `to_json(parse(to_json(x))) == to_json(x)`. A codec that dropped or
//!    reordered a field would break this. (`DepositMsg` has no `PartialEq`, so
//!    we compare the re-serialized JSON rather than the value.)
//!
//! Replaces a toy target that only built hardcoded format-string JSON and
//! asserted `str::contains` / `saturating_add` properties — no contract code
//! (P1 violation).
//!
//! MUTATION-CHECK (P5): rename the `#[serde]` tag of a `DepositMsg` variant
//! (e.g. add `#[serde(rename = "Liquidate2")]` to `Liquidate`). Then a
//! constructed `Liquidate` re-serializes to a key the parser no longer accepts
//! and the round-trip `from_str` below fails (or the idempotence assert fires).

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use std::str::FromStr;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use near_sdk::{serde_json, AccountId};
use templar_common::{
    asset::CollateralAssetAmount,
    market::{DepositMsg, LiquidateMsg, RepayAccountMsg},
};

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    /// Arbitrary bytes fed to the raw parser (oracle 1).
    raw: &'a [u8],
    /// Selects which `DepositMsg` variant to build for the round-trip (oracle 2).
    variant: u8,
    /// Account-name bytes for the variants that carry an `AccountId`.
    account: &'a [u8],
    /// `Some` amount for `Liquidate`, or whole-position liquidation when `None`.
    amount: Option<u128>,
}

/// Build a valid NEAR `AccountId` from arbitrary bytes, or `None`.
fn account_id_from(bytes: &[u8]) -> Option<AccountId> {
    let name: String = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == '_')
        .take(48)
        .collect();
    if name.is_empty() {
        return None;
    }
    AccountId::from_str(&format!("{name}.near")).ok()
}

fuzz_target!(|input: Input| {
    // Oracle 1: raw parse of untrusted bytes must never panic.
    if let Ok(s) = std::str::from_utf8(input.raw) {
        let _ = serde_json::from_str::<DepositMsg>(s);
    }

    // Oracle 2: round-trip idempotence for a constructed message.
    let msg = match input.variant % 5 {
        0 => DepositMsg::Supply,
        1 => DepositMsg::Collateralize,
        2 => DepositMsg::Repay,
        3 => match account_id_from(input.account) {
            Some(account_id) => DepositMsg::RepayAccount(RepayAccountMsg { account_id }),
            None => return,
        },
        _ => match account_id_from(input.account) {
            Some(account_id) => DepositMsg::Liquidate(LiquidateMsg {
                account_id,
                amount: input.amount.map(CollateralAssetAmount::new),
            }),
            None => return,
        },
    };

    let json1 = serde_json::to_string(&msg).expect("DepositMsg must serialize");
    let parsed: DepositMsg =
        serde_json::from_str(&json1).expect("DepositMsg must re-parse its own JSON");
    let json2 = serde_json::to_string(&parsed).expect("re-parsed DepositMsg must serialize");
    assert_eq!(
        json1, json2,
        "DepositMsg JSON round-trip is not idempotent",
    );
});
