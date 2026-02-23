use near_sdk::borsh::{self, io};
use primitive_types::U256;

#[derive(Debug, Clone)]
#[near_sdk::near(serializers = [json, borsh])]
pub struct FeedData {
    #[borsh(serialize_with = "u256_borsh_ser", deserialize_with = "u256_borsh_de")]
    pub price: U256,
    pub package_timestamp: u64,
    pub write_timestamp: u64,
}

fn u256_borsh_ser<W: io::Write>(x: &U256, writer: &mut W) -> Result<(), io::Error> {
    borsh::BorshSerialize::serialize(&x.0, writer)
}

fn u256_borsh_de<R: io::Read>(reader: &mut R) -> Result<U256, io::Error> {
    Ok(U256(borsh::BorshDeserialize::deserialize_reader(reader)?))
}
