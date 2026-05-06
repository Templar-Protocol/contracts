#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "schemars")]
pub(crate) use alloc::borrow::ToOwned;
#[cfg(feature = "schemars")]
pub(crate) use alloc::boxed::Box;
#[cfg(any(feature = "borsh", feature = "schemars"))]
pub(crate) use alloc::format;
#[cfg(any(feature = "borsh", feature = "schemars"))]
pub(crate) use alloc::string::ToString;
#[cfg(feature = "schemars")]
pub(crate) use alloc::vec;

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
