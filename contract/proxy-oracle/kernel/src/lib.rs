#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;
#[cfg(not(feature = "std"))]
pub(crate) use alloc as std;

#[cfg(feature = "schemars")]
pub(crate) use crate::std::borrow::ToOwned;
#[cfg(feature = "schemars")]
pub(crate) use crate::std::boxed::Box;
#[cfg(feature = "schemars")]
pub(crate) use crate::std::format;
#[cfg(any(feature = "borsh", feature = "schemars"))]
pub(crate) use crate::std::string::ToString;
#[cfg(feature = "schemars")]
pub(crate) use crate::std::vec;

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
