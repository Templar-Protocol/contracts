use near_sdk::{near, serde};

#[test]
fn key() {
    use stellar_strkey::ed25519::PublicKey;
    let k = stellar_strkey::Strkey::PublicKeyEd25519(PublicKey([0xffu8; 32]));
    let y = PublicKey([0xffu8; 32]);

    eprintln!("{k}");
    eprintln!("{y}");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [borsh])]
pub struct PublicKey(pub [u8; 32]);

impl serde::Serialize for PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        stellar_strkey::ed25519::PublicKey(self.0).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = stellar_strkey::ed25519::PublicKey::deserialize(deserializer)?.0;
        Ok(Self(bytes))
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
