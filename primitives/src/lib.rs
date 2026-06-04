#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod number;
pub use number::Decimal;
pub mod strnum;
pub use strnum::{SI128, SI64, SU128, SU256, SU64};
pub mod time;
pub use time::Nanoseconds;
