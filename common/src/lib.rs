pub mod accumulator;
pub mod asset;
pub mod borrow;
pub mod chunked_append_only_list;
pub mod event;
pub mod fee;
pub mod incoming_deposit;
pub mod interest_rate_strategy;
pub mod market;
pub mod number;
pub mod oracle;
pub mod price;
pub mod registry;
pub mod snapshot;
pub mod supply;
pub mod time_chunk;
pub mod vault;
pub mod withdrawal_queue;

pub use primitive_types;

/// Approximation of `1 / (1000 * 60 * 60 * 24 * 365.2425)`.
///
/// exact = 0.00000000003168873850681143096456210346297...
/// this  = 0.00000000003168873850681143096456210346
///
/// error =~ 9.375e-27 %
pub static YEAR_PER_MS: number::Decimal =
    number::Decimal::from_repr([0x40FC_AB61_4AE4_B2B5, 0x22D7_9641, 0, 0, 0, 0, 0, 0]);

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
