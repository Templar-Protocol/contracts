use std::str::FromStr;

use alloy::signers::Signature as AlloySignature;
use near_sdk::serde::{self, Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct Signature(pub AlloySignature);

impl From<AlloySignature> for Signature {
    fn from(value: AlloySignature) -> Self {
        Self(value)
    }
}

impl From<Signature> for AlloySignature {
    fn from(value: Signature) -> Self {
        value.0
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        <String as serde::Serialize>::serialize(&self.0.to_string(), serializer)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        let sig = AlloySignature::from_str(&s).map_err(serde::de::Error::custom)?;
        Ok(Self(sig))
    }
}

impl schemars::JsonSchema for Signature {
    fn schema_name() -> String {
        "Signature".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Ethereum signature".to_string());
        schema.string().pattern = Some("^0x[0-9a-fA-F]{130}$".to_string());
        schema.into()
    }
}

#[cfg(test)]
mod tests {
    use alloy::signers::{local::PrivateKeySigner, SignerSync};
    use near_sdk::serde_json;

    use super::*;

    fn key1() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(
            &[
                202, 109, 4, 134, 189, 42, 105, 41, 29, 134, 213, 198, 255, 149, 228, 40, 14, 202,
                187, 103, 63, 234, 214, 21, 114, 123, 76, 247, 70, 175, 215, 170,
            ]
            .into(),
        )
        .unwrap()
    }

    fn sig1(msg: &[u8]) -> Signature {
        let sig = key1().sign_message_sync(msg).unwrap();
        Signature(sig)
    }

    fn key2() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(
            &[
                202, 109, 4, 134, 189, 42, 105, 41, 29, 134, 213, 198, 255, 149, 228, 40, 14, 202,
                187, 103, 63, 234, 214, 21, 114, 123, 76, 247, 70, 175, 215, 171,
            ]
            .into(),
        )
        .unwrap()
    }

    fn sig2(msg: &[u8]) -> Signature {
        let keypair = key2();
        let sig = keypair.sign_message_sync(msg).unwrap();
        Signature(sig)
    }

    #[test]
    fn json_serialization() {
        let sig1 = sig1(b"abc");
        let json1 = serde_json::to_string(&sig1).unwrap();
        assert_eq!(
            json1,
            r#""0xb12461f100937e17f066aee467c767e7dd0db7968c26b36e46d7abc8f4a080872d42ea30aba81b2b82d53c1a959b4da932bb6bbf83383665ef84fc62bddf24741b""#,
        );
        let parsed1 = serde_json::from_str(&json1).unwrap();
        assert_eq!(sig1, parsed1);

        let sig2 = sig2(b"abc");
        let json2 = serde_json::to_string(&sig2).unwrap();
        assert_eq!(
            json2,
            r#""0x57628d39b64a8b63663d01ca84e5aa9d21ed55b6d263d3e138704b73a9c6452e72cc571e6c3b2677287a3b2bcc35e04f1af8a26bd050d9276bdb214fcd50f05a1b""#,
        );
        let parsed2 = serde_json::from_str(&json2).unwrap();
        assert_eq!(sig2, parsed2);
    }
}
