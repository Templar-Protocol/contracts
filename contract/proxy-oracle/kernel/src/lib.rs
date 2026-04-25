#![no_std]

#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;
#[cfg(not(feature = "std"))]
pub(crate) use alloc as std;

use std::borrow::ToOwned;
use std::boxed::Box;
use std::format;
use std::string::String;
use std::string::ToString;
use std::vec;
use std::vec::Vec;

macro_rules! serialize {
    ($i: item) => {
        #[cfg_attr(
            feature = "borsh",
            derive(
                ::borsh::BorshSerialize,
                ::borsh::BorshDeserialize,
                ::borsh::BorshSchema
            )
        )]
        #[cfg_attr(feature = "schemars", derive(::schemars::JsonSchema))]
        #[cfg_attr(feature = "serde", derive(::serde::Serialize, ::serde::Deserialize))]
        $i
    };
}

mod price;
pub use price::Price;
pub mod primitive;
pub mod proxy;
