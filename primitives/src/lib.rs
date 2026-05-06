#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod imports {
    pub use alloc::format;
    pub use alloc::string::String;
    pub use alloc::string::ToString;
    pub use alloc::vec::Vec;
}
pub(crate) use imports::*;

pub mod number;
pub use number::Decimal;
pub mod strnum;
pub use strnum::{SI128, SI64, SU128, SU256, SU64};
pub mod time;
pub use time::Nanoseconds;
