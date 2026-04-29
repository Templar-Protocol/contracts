#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;
#[cfg(not(feature = "std"))]
pub(crate) use alloc as std;

#[doc(hidden)]
pub mod derive_prelude {
    pub use crate::std::borrow::ToOwned;
    pub use crate::std::boxed::Box;
    pub use crate::std::format;
    pub use crate::std::string::ToString;
    pub use crate::std::vec;
}

macro_rules! serialize {
    ($i: item) => {
        #[allow(unused_imports)]
        use crate::derive_prelude::*;

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
