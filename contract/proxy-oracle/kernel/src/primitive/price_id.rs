use crate::*;

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

#[cfg(test)]
mod tests {
    use super::PriceIdentifier;

    #[cfg(feature = "serde")]
    #[test]
    fn serde_round_trip_uses_lowercase_hex_string() {
        let id = PriceIdentifier([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x01, 0x23, 0x45, 0x67,
            0x89, 0xab, 0xcd, 0xef,
        ]);

        let serialized = serde_json::to_string(&id).unwrap();
        assert_eq!(
            serialized,
            r#""00112233445566778899aabbccddeeff1032547698badcfe0123456789abcdef""#
        );

        let deserialized: PriceIdentifier = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, id);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_rejects_malformed_hex() {
        assert!(serde_json::from_str::<PriceIdentifier>(r#""xyz""#).is_err());
        assert!(serde_json::from_str::<PriceIdentifier>(r#""0011""#).is_err());
    }
}
