pub mod accumulator;
pub mod asset;
pub mod borrow;
pub mod chunked_append_only_list;
pub mod event;
pub mod fee;
pub mod governance;
pub mod guard;
pub mod incoming_deposit;
pub mod interest_rate_strategy;
pub mod market;
pub mod oracle;
pub mod price;
pub mod registry;
pub mod snapshot;
pub mod supply;
pub mod time_chunk;
#[cfg(feature = "rpc")]
pub mod utils;
pub mod vault;
pub mod versioned_state;
pub mod withdrawal_queue;

pub use primitive_types;
pub use schemars;
pub use templar_primitives::dec;
pub use templar_primitives::{Decimal, Nanoseconds, SI128, SI64, SU128, SU256, SU64};

/// Panic helper that works in both WASM and native contexts.
///
/// In WASM contexts (contract compilation), uses `near_sdk::env::panic_str`.
/// In native contexts (bots, tests), uses standard `panic!`.
#[cfg(target_arch = "wasm32")]
#[inline]
pub fn panic_with_message(msg: &str) -> ! {
    near_sdk::env::panic_str(msg);
}

/// Panic helper that works in both WASM and native contexts.
///
/// In WASM contexts (contract compilation), uses `near_sdk::env::panic_str`.
/// In native contexts (bots, tests), uses standard `panic!`.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub fn panic_with_message(msg: &str) -> ! {
    panic!("{}", msg);
}

/// Extension trait for `Option` and `Result` that panics with a custom message on failure.
pub trait UnwrapReject<T> {
    /// Unwraps the value with a default panic message.
    fn unwrap_or_reject(self) -> T;
    /// Unwraps the value with a custom panic message.
    fn expect_or_reject(self, msg: &str) -> T;
}

impl<T> UnwrapReject<T> for Option<T> {
    fn unwrap_or_reject(self) -> T {
        self.expect_or_reject("called `Option::unwrap_or_reject()` on a `None` value")
    }

    fn expect_or_reject(self, msg: &str) -> T {
        match self {
            Some(value) => value,
            None => panic_with_message(msg),
        }
    }
}

impl<T, E: std::fmt::Display> UnwrapReject<T> for Result<T, E> {
    fn unwrap_or_reject(self) -> T {
        self.expect_or_reject("called `Result::unwrap_or_reject()` on an `Err` value")
    }

    fn expect_or_reject(self, msg: &str) -> T {
        match self {
            Ok(value) => value,
            Err(err) => panic_with_message(&format!("{msg}: {err}")),
        }
    }
}

/// Approximation of `1 / (1000 * 60 * 60 * 24 * 365.2425)`.
///
/// exact = 0.00000000003168873850681143096456210346297...
/// this  = 0.00000000003168873850681143096456210346
///
/// error =~ 9.375e-27 %
pub static YEAR_PER_MS: Decimal =
    Decimal::from_repr([0x40FC_AB61_4AE4_B2B5, 0x22D7_9641, 0, 0, 0, 0, 0, 0]);

pub mod contract {
    pub fn list<T, U: FromIterator<T>>(
        i: impl IntoIterator<Item = T>,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> U {
        let offset = offset.map_or(0, |o| o as usize);
        let count = count.map_or(usize::MAX, |c| c as usize);
        i.into_iter().skip(offset).take(count).collect()
    }

    #[macro_export]
    macro_rules! self_ext {
        ($gas:expr) => {
            Self::ext(::near_sdk::env::current_account_id()).with_static_gas($gas)
        };
    }
}
