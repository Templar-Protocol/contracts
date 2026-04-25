#![no_std]

#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;
#[cfg(not(feature = "std"))]
pub(crate) use alloc as std;

mod imports {
    use crate::std;

    pub use std::format;
    pub use std::string::String;
    pub use std::string::ToString;
    pub use std::vec::Vec;
}
pub(crate) use imports::*;

pub mod number;
pub use number::Decimal;
pub mod strnum;
pub use strnum::{SI128, SI64, SU128, SU256, SU64};
pub mod time;
pub use time::Nanoseconds;
