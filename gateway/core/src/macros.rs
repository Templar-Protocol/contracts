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

macro_rules! public_read_method_spec {
    ($name:ident, $rpc_method_name:literal, $method_identifier:expr, $input:ty, $output:ty) => {
        pub struct $name;

        impl $crate::MethodSpec for $name {
            type Input = $crate::common::ReadRequest<$input>;
            type Output = $output;

            const RPC_METHOD: &'static str = $rpc_method_name;
            const IDENTIFIER: $crate::method::MethodSelector =
                $crate::method::MethodSelector::Read($method_identifier);
        }
    };
}

macro_rules! write_method_spec {
    ($name:ident, $rpc_method_name:literal, $method_identifier:expr, $input:ty, $output:ty) => {
        pub struct $name;

        impl $crate::MethodSpec for $name {
            type Input = $crate::common::WriteRequest<$input>;
            type Output = $output;

            const RPC_METHOD: &'static str = $rpc_method_name;
            const IDENTIFIER: $crate::method::MethodSelector =
                $crate::method::MethodSelector::Write($method_identifier);
        }
    };
}

pub(crate) use public_read_method_spec;
pub(crate) use transparent_newtype;
pub(crate) use write_method_spec;
