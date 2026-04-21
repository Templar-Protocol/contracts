macro_rules! transparent_newtype {
    ($(#[$meta:meta])* $vis:vis struct $name:ident($inner:ty);) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
            schemars::JsonSchema,
        )]
        #[serde(transparent)]
        $vis struct $name(pub $inner);
    };
}

pub(crate) use blockchain_gateway_macros::{public_read_method_spec, write_method_spec};
pub(crate) use transparent_newtype;
