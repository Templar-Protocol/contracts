pub mod accumulator;
pub mod asset;
pub mod borrow;
pub mod chunked_append_only_list;
pub mod event;
pub mod fee;
pub mod interest_rate_strategy;
pub mod market;
pub mod number;
pub mod oracle;
pub mod snapshot;
pub mod static_yield;
pub mod supply;
pub mod time_chunk;
pub mod withdrawal_queue;

pub const MS_IN_A_YEAR: u128 = 31_556_952_000; // 1000 * 60 * 60 * 24 * 365.2425

#[macro_export]
macro_rules! self_ext {
    () => {
        Self::ext(::near_sdk::env::current_account_id())
    };
}
