serialize! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct PriceIdentifier(
        #[cfg_attr(feature = "serde", serde(
            serialize_with = "hex::serde::serialize",
            deserialize_with = "hex::serde::deserialize"
        ))]
        pub [u8; 32],
    );
}
