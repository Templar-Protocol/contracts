#[cfg(feature = "schemars")]
use alloc::string::String;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;

const PRICE_IDENTIFIER_BYTES: usize = 32;
#[cfg(feature = "schemars")]
const PRICE_IDENTIFIER_HEX_LENGTH: u32 = 64;
#[cfg(feature = "schemars")]
const PRICE_IDENTIFIER_HEX_PATTERN: &str = "^[0-9A-Fa-f]{64}$";

#[cfg_attr(
    feature = "borsh",
    derive(
        ::borsh::BorshSerialize,
        ::borsh::BorshDeserialize,
        ::borsh::BorshSchema
    )
)]
#[cfg_attr(feature = "serde", derive(::serde::Serialize, ::serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PriceIdentifier(
    #[cfg_attr(
        feature = "serde",
        serde(
            serialize_with = "hex::serde::serialize",
            deserialize_with = "hex::serde::deserialize"
        )
    )]
    pub [u8; PRICE_IDENTIFIER_BYTES],
);

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for PriceIdentifier {
    fn schema_name() -> String {
        "PriceIdentifier".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        let validation = schema.string();
        validation.min_length = Some(PRICE_IDENTIFIER_HEX_LENGTH);
        validation.max_length = Some(PRICE_IDENTIFIER_HEX_LENGTH);
        validation.pattern = Some(PRICE_IDENTIFIER_HEX_PATTERN.to_string());
        schema.into()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "serde", feature = "schemars", feature = "borsh"))]
    use super::PriceIdentifier;
    #[cfg(feature = "borsh")]
    use super::PRICE_IDENTIFIER_BYTES;
    #[cfg(feature = "schemars")]
    use super::{PRICE_IDENTIFIER_HEX_LENGTH, PRICE_IDENTIFIER_HEX_PATTERN};

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

    #[cfg(feature = "serde")]
    #[test]
    fn hal_26_serde_accepts_exact_case_insensitive_hex_only() {
        let valid = [
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "AaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAa",
        ];
        for value in valid {
            let encoded = alloc::format!("\"{value}\"");
            assert!(serde_json::from_str::<PriceIdentifier>(&encoded).is_ok());
        }

        let invalid = [
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "gaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            " aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "ＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡＡ",
        ];
        for value in invalid {
            let encoded = alloc::format!("\"{value}\"");
            assert!(serde_json::from_str::<PriceIdentifier>(&encoded).is_err());
        }
        assert!(serde_json::from_str::<PriceIdentifier>("[0, 1]").is_err());
    }

    #[cfg(feature = "schemars")]
    #[test]
    fn hal_26_schema_describes_the_hex_wire_format() {
        let schema = schemars::schema_for!(PriceIdentifier);
        let validation = schema.schema.string.as_ref().unwrap();
        assert_eq!(validation.min_length, Some(PRICE_IDENTIFIER_HEX_LENGTH));
        assert_eq!(validation.max_length, Some(PRICE_IDENTIFIER_HEX_LENGTH));
        assert_eq!(
            validation.pattern.as_deref(),
            Some(PRICE_IDENTIFIER_HEX_PATTERN)
        );
        assert!(schema.schema.array.is_none());
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn hal_26_borsh_bytes_remain_unchanged() {
        let bytes = core::array::from_fn(|index| {
            u8::try_from(index).unwrap_or_else(|_| unreachable!("array length is below u8::MAX"))
        });
        let id = PriceIdentifier(bytes);
        assert_eq!(borsh::to_vec(&id).unwrap(), bytes);
        assert_eq!(borsh::to_vec(&id).unwrap().len(), PRICE_IDENTIFIER_BYTES);
    }
}
