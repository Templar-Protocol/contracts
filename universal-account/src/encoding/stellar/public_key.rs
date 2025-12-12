use std::{ops::Deref, str::FromStr};

use near_sdk::{
    near,
    serde::{Deserialize, Serialize},
};
use stellar_strkey::ed25519::PublicKey as StellarPublicKey;

type ByteEncoding = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(
    crate = "near_sdk::serde",
    from = "StellarPublicKey",
    into = "StellarPublicKey"
)]
#[near(serializers = [borsh])]
pub struct PublicKey(pub ByteEncoding);

impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&StellarPublicKey(self.0), f)
    }
}

impl Deref for PublicKey {
    type Target = ByteEncoding;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<StellarPublicKey> for PublicKey {
    fn from(value: StellarPublicKey) -> Self {
        Self(value.0)
    }
}

impl From<PublicKey> for StellarPublicKey {
    fn from(value: PublicKey) -> Self {
        StellarPublicKey(value.0)
    }
}

impl From<ByteEncoding> for PublicKey {
    fn from(value: ByteEncoding) -> Self {
        Self(value)
    }
}

impl From<PublicKey> for ByteEncoding {
    fn from(value: PublicKey) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<ByteEncoding> for PublicKey {
    fn as_ref(&self) -> &ByteEncoding {
        &self.0
    }
}

impl FromStr for PublicKey {
    type Err = <StellarPublicKey as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(StellarPublicKey::from_str(s)?.0))
    }
}

impl schemars::JsonSchema for PublicKey {
    fn schema_name() -> String {
        "PublicKey".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Stellar public key".to_string());
        schema.string().pattern = Some("^G[A-Z2-7]{55}$".to_string());
        schema.into()
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;
    use soroban_client::keypair::{Keypair, KeypairBehavior};

    use super::*;

    fn public_key(keypair: &Keypair) -> super::PublicKey {
        super::PublicKey(keypair.raw_public_key().clone().try_into().unwrap())
    }

    #[test]
    fn borsh_serialization() {
        let keypair = Keypair::random().unwrap();
        let keypair_2 = Keypair::random().unwrap();
        let pubkey = public_key(&keypair);
        let pubkey_2 = public_key(&keypair_2);

        eprintln!("{pubkey}");

        assert_ne!(pubkey, pubkey_2);

        let borsh_ser = borsh::to_vec(&pubkey).unwrap();
        let borsh_ser_2 = borsh::to_vec(&pubkey_2).unwrap();

        assert_ne!(borsh_ser, borsh_ser_2);

        let parsed: super::PublicKey = borsh::from_slice(&borsh_ser).unwrap();
        let parsed_2: super::PublicKey = borsh::from_slice(&borsh_ser_2).unwrap();

        assert_ne!(parsed, parsed_2);

        assert_eq!(pubkey, parsed);
        assert_eq!(pubkey_2, parsed_2);
    }

    #[test]
    fn json_serialization() {
        let keypair = Keypair::random().unwrap();
        let keypair_2 = Keypair::random().unwrap();
        let pubkey = public_key(&keypair);
        let pubkey_2 = public_key(&keypair_2);

        assert_ne!(pubkey, pubkey_2);

        let json_ser = serde_json::to_string(&pubkey).unwrap();
        let json_ser_2 = serde_json::to_string(&pubkey_2).unwrap();

        assert_ne!(json_ser, json_ser_2);

        let parsed: super::PublicKey = serde_json::from_str(&json_ser).unwrap();
        let parsed_2: super::PublicKey = serde_json::from_str(&json_ser_2).unwrap();

        assert_ne!(parsed, parsed_2);

        assert_eq!(pubkey, parsed);
        assert_eq!(pubkey_2, parsed_2);
    }

    #[test]
    fn to_from_string() {
        let keypair = Keypair::random().unwrap();
        let pubkey = public_key(&keypair);
        let pk_str = pubkey.to_string();

        let parsed = super::PublicKey::from_str(&pk_str).unwrap();

        assert_eq!(parsed, pubkey);

        let keypair_2 = Keypair::random().unwrap();
        let pk_str_2 = public_key(&keypair_2).to_string();

        assert_ne!(pk_str, pk_str_2);
    }

    #[test]
    fn from_str() {
        let s = "GDCXO2FCO2KMI2NH23WSBAY5WZDE3LJUIZLKMBD4YIQEA4EA7LCJXPJP";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "Invalid"]
    fn from_str_fail_lowercase() {
        let s = "gdcxo2fco2kmi2nh23wsbay5wzde3ljuizlkmbd4yiqea4ea7lcjxpjp";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "Invalid"]
    fn from_str_fail_long() {
        let s = "GDCXO2FCO2KMI2NH23WSBAY5WZDE3LJUIZLKMBD4YIQEA4EA7LCJXPJPA";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "Invalid"]
    fn from_str_fail_short() {
        let s = "GDCXO2FCO2KMI2NH23WSBAY5WZDE3LJUIZLKMBD4YIQEA4EA7LCJXPJ";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "Invalid"]
    fn from_str_fail_character() {
        let s = "GDCXO2FCO2KMI2NH23WSBAY5WZDE3LJUIZLKMBD4YIQEA4EA7LCJXPJ1";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "Invalid"]
    fn from_str_fail_prefix() {
        let s = "DGCXO2FCO2KMI2NH23WSBAY5WZDE3LJUIZLKMBD4YIQEA4EA7LCJXPJP";
        super::PublicKey::from_str(s).unwrap();
    }
}
