use std::str::FromStr;

use alloy::signers::Signature as AlloySignature;
use borsh::{BorshDeserialize, BorshSerialize};
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

impl BorshSerialize for Signature {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        BorshSerialize::serialize(&self.0.as_bytes(), writer)
    }
}

impl BorshDeserialize for Signature {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let bytes = <[u8; 65] as BorshDeserialize>::deserialize_reader(reader)?;
        let sig = AlloySignature::from_raw_array(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        Ok(Self(sig))
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

    #[test]
    fn borsh_serialization() {
        let sig1 = sig1(b"abc");
        let borsh1 = borsh::to_vec(&sig1).unwrap();
        assert_eq!(
            borsh1,
            b"\xb1\x24\x61\xf1\x00\x93\x7e\x17\xf0\x66\xae\xe4\x67\xc7\x67\xe7\xdd\x0d\xb7\x96\x8c\x26\xb3\x6e\x46\xd7\xab\xc8\xf4\xa0\x80\x87\x2d\x42\xea\x30\xab\xa8\x1b\x2b\x82\xd5\x3c\x1a\x95\x9b\x4d\xa9\x32\xbb\x6b\xbf\x83\x38\x36\x65\xef\x84\xfc\x62\xbd\xdf\x24\x74\x1b",
        );
        let parsed1 = borsh::from_slice(&borsh1).unwrap();
        assert_eq!(sig1, parsed1);

        let sig2 = sig2(b"abc");
        let borsh2 = borsh::to_vec(&sig2).unwrap();
        assert_eq!(
            borsh2,
            b"\x57\x62\x8d\x39\xb6\x4a\x8b\x63\x66\x3d\x01\xca\x84\xe5\xaa\x9d\x21\xed\x55\xb6\xd2\x63\xd3\xe1\x38\x70\x4b\x73\xa9\xc6\x45\x2e\x72\xcc\x57\x1e\x6c\x3b\x26\x77\x28\x7a\x3b\x2b\xcc\x35\xe0\x4f\x1a\xf8\xa2\x6b\xd0\x50\xd9\x27\x6b\xdb\x21\x4f\xcd\x50\xf0\x5a\x1b",
        );
        let parsed2 = borsh::from_slice(&borsh2).unwrap();
        assert_eq!(sig2, parsed2);
    }
}
