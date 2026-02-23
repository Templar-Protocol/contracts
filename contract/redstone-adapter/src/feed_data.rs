#[derive(Debug, Clone)]
#[near_sdk::near(serializers = [json, borsh])]
pub struct FeedData {
    #[borsh(
        serialize_with = "u256_borsh::serialize",
        deserialize_with = "u256_borsh::deserialize"
    )]
    pub price: primitive_types::U256,
    pub package_timestamp: u64,
    pub write_timestamp: u64,
}

mod u256_borsh {
    use near_sdk::borsh;

    pub fn serialize<W: borsh::io::Write>(
        obj: &primitive_types::U256,
        writer: &mut W,
    ) -> ::core::result::Result<(), borsh::io::Error> {
        borsh::BorshSerialize::serialize(&obj.0, writer)
    }

    pub fn deserialize<R: borsh::io::Read>(
        reader: &mut R,
    ) -> ::core::result::Result<primitive_types::U256, borsh::io::Error> {
        Ok(primitive_types::U256(
            borsh::BorshDeserialize::deserialize_reader(reader)?,
        ))
    }
}
