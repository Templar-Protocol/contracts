use alloy::signers::Signature as AlloySignature;
use near_sdk::serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
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

#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(remote = "AlloySignature")]
struct RemoteAlloySignature {
    pub r: String,
    pub s: String,
    #[serde(rename = "yParity")]
    pub y_parity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<String>,
}

impl schemars::JsonSchema for Signature {
    fn schema_name() -> String {
        <RemoteAlloySignature as schemars::JsonSchema>::schema_name()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        <RemoteAlloySignature as schemars::JsonSchema>::json_schema(gen)
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
            r#"{"r":"0xb12461f100937e17f066aee467c767e7dd0db7968c26b36e46d7abc8f4a08087","s":"0x2d42ea30aba81b2b82d53c1a959b4da932bb6bbf83383665ef84fc62bddf2474","yParity":"0x0","v":"0x0"}"#,
        );
        let parsed1 = serde_json::from_str(&json1).unwrap();
        assert_eq!(sig1, parsed1);

        let sig2 = sig2(b"abc");
        let json2 = serde_json::to_string(&sig2).unwrap();
        assert_eq!(
            json2,
            r#"{"r":"0x57628d39b64a8b63663d01ca84e5aa9d21ed55b6d263d3e138704b73a9c6452e","s":"0x72cc571e6c3b2677287a3b2bcc35e04f1af8a26bd050d9276bdb214fcd50f05a","yParity":"0x0","v":"0x0"}"#,
        );
        let parsed2 = serde_json::from_str(&json2).unwrap();
        assert_eq!(sig2, parsed2);
    }
}
