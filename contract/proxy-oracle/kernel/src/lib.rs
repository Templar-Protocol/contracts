#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

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
