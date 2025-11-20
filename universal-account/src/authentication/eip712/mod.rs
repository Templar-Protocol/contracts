use alloy::primitives::Address;
use borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use near_sdk::near;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [])]
pub struct VerifyKey(Address);

// impl BorshSerialize for VerifyKey {
//     fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
//         let bytes: [u8; 20] = self.0.into();
//         BorshSerialize::serialize(&bytes, writer)
//     }
// }

// impl BorshDeserialize for VerifyKey {
//     fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
//         let bytes = <[u8; 20] as BorshDeserialize>::deserialize_reader(reader)?;
//         Ok(Self(Address::from(bytes)))
//     }
// }

// impl BorshSchema for VerifyKey {
//     fn add_definitions_recursively(
//         definitions: &mut std::collections::BTreeMap<
//             near_sdk::borsh::schema::Declaration,
//             near_sdk::borsh::schema::Definition,
//         >,
//     ) {
//         // <[u64; 8] as BorshSchema>::add_definitions_recursively(definitions);
//         todo!()
//     }

//     fn declaration() -> near_sdk::borsh::schema::Declaration {
//         // String::from("Decimal")
//         todo!()
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization() {
        // let key = VerifyKey(Address::from_slice(&[0xff; 20]));

        // eprintln!("{}", near_sdk::serde_json::to_string(&key).unwrap());
    }
}
