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
pub mod price;
pub mod snapshot;
pub mod static_yield;
pub mod supply;
pub mod time_chunk;
pub mod withdrawal_queue;

pub static MS_IN_A_YEAR: std::sync::LazyLock<number::Decimal> =
    std::sync::LazyLock::new(|| number::Decimal::from(31_556_952_000_u128)); // 1000 * 60 * 60 * 24 * 365.2425

#[macro_export]
macro_rules! define_list {
    ($v:vis fn $n:ident (&$self:ident) -> $ret:ty { $it:expr } $($tail:tt)*) => {
        $v fn $n(&$self, offset: Option<u32>, count: Option<u32>) -> $ret {
            let offset = offset.map_or(0, |o| o as usize);
            let count = count.map_or(usize::MAX, |c| c as usize);
            ($it).into_iter().skip(offset).take(count).collect()
        }

        $crate::define_list! { $($tail)* }
    };
    () => {};
}
