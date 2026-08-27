use std::path::{Path, PathBuf};

use anyhow::Context as _;
use base64::Engine as _;
use borsh::BorshSerialize;
use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{
    de::{DeserializeOwned, Error as _},
    Deserialize, Deserializer, Serialize, Serializer,
};
use sha2::{Digest as _, Sha256};
use templar_gateway_types::Base64Bytes;

pub const PATCH_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchSpec {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<PathBuf>,
    pub account_id: AccountId,
    #[serde(default, rename = "op")]
    pub ops: Vec<PatchOperation>,
    #[serde(default, rename = "check")]
    pub checks: Vec<PatchCheck>,
}

impl PatchSpec {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut spec: Self =
            super::extends::load_raw(path, PATCH_SCHEMA_VERSION, "patch plan", "")?;
        spec.extends.clear();
        Ok(spec)
    }

    pub fn resolve(&self, source_path: &Path) -> anyhow::Result<ResolvedPatch> {
        let base = source_path.parent().unwrap_or_else(|| Path::new("."));
        let mut ops = Vec::with_capacity(self.checks.len() + self.ops.len());
        for check in &self.checks {
            ops.push(ResolvedOperation::Expect {
                key: Base64Bytes(check.key.resolve(base)?),
                expected: check.expect.resolve(base)?,
            });
        }
        for operation in &self.ops {
            ops.push(operation.resolve(base)?);
        }
        Ok(ResolvedPatch { operations: ops })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchCheck {
    pub key: ByteExpr,
    pub expect: Expectation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PatchOperation {
    Set {
        key: ByteExpr,
        value: ByteExpr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<Expectation>,
    },
    Remove {
        remove: ByteExpr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<Expectation>,
    },
    RemovePrefix {
        remove_prefix: ByteExpr,
    },
}

impl PatchOperation {
    fn resolve(&self, base: &Path) -> anyhow::Result<ResolvedOperation> {
        match self {
            Self::Set { key, value, expect } => Ok(ResolvedOperation::Set {
                key: Base64Bytes(key.resolve(base)?),
                value: Base64Bytes(value.resolve(base)?),
                expected: expect
                    .as_ref()
                    .map(|expect| expect.resolve(base))
                    .transpose()?,
            }),
            Self::Remove { remove, expect } => Ok(ResolvedOperation::Remove {
                key: Base64Bytes(remove.resolve(base)?),
                expected: expect
                    .as_ref()
                    .map(|expect| expect.resolve(base))
                    .transpose()?,
            }),
            Self::RemovePrefix { remove_prefix } => Ok(ResolvedOperation::RemovePrefix {
                prefix: Base64Bytes(remove_prefix.resolve(base)?),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ByteExpr {
    Utf8(String),
    Hex(String),
    Base64(String),
    File(PathBuf),
    Concat(Vec<ByteExpr>),
    Sha256(Box<ByteExpr>),
    Json(serde_json::Value),
    Borsh(BorshExpr),
}

impl ByteExpr {
    pub fn resolve(&self, base: &Path) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Utf8(value) => Ok(value.as_bytes().to_vec()),
            Self::Hex(value) => hex::decode(value).context("decode hex byte expression"),
            Self::Base64(value) => base64::engine::general_purpose::STANDARD
                .decode(value)
                .context("decode base64 byte expression"),
            Self::File(path) => std::fs::read(base.join(path)).with_context(|| {
                format!("read byte expression file {}", base.join(path).display())
            }),
            Self::Concat(values) => values.iter().try_fold(Vec::new(), |mut bytes, value| {
                bytes.extend(value.resolve(base)?);
                Ok(bytes)
            }),
            Self::Sha256(value) => Ok(Sha256::digest(value.resolve(base)?).to_vec()),
            Self::Json(value) => serde_json::to_vec(value).context("encode JSON byte expression"),
            Self::Borsh(value) => value.resolve(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BorshExpr {
    #[serde(rename = "type")]
    pub type_name: String,
    pub value: serde_json::Value,
}

struct Codec {
    name: &'static str,
    encode: fn(&serde_json::Value, &str) -> anyhow::Result<Vec<u8>>,
}

fn encode<T: DeserializeOwned + BorshSerialize>(
    value: &serde_json::Value,
    type_name: &str,
) -> anyhow::Result<Vec<u8>> {
    let value = T::deserialize(value).with_context(|| format!("parse borsh {type_name} value"))?;
    borsh::to_vec(&value).with_context(|| format!("encode borsh {type_name}"))
}

macro_rules! codecs {
    ($($name:literal => $type:ty),+ $(,)?) => {
        static CODECS: &[Codec] = &[
            $(Codec { name: $name, encode: encode::<$type> }),+
        ];
    };
}

codecs!(
    "AccountId" => AccountId,
    "String" => String,
    "Vec<u8>" => Vec<u8>,
    "bool" => bool,
    "u128" => u128,
    "u32" => u32,
    "u64" => u64,
    "u8" => u8,
);

impl BorshExpr {
    fn resolve(&self) -> anyhow::Result<Vec<u8>> {
        let Some(codec) = CODECS.iter().find(|codec| codec.name == self.type_name) else {
            anyhow::bail!(
                "unknown borsh codec `{}`; run `tmplrmgr patch codecs` for supported types",
                self.type_name
            );
        };
        (codec.encode)(&self.value, &self.type_name)
    }
}

pub fn codec_names() -> impl Iterator<Item = &'static str> {
    CODECS.iter().map(|codec| codec.name)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sha256Digest(pub [u8; 32]);

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.len() != 64 || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(D::Error::custom(
                "SHA-256 digest must be exactly 64 lowercase hex characters",
            ));
        }
        let bytes = hex::decode(&value).map_err(D::Error::custom)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| D::Error::custom("SHA-256 digest must decode to exactly 32 bytes"))?;
        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Expectation {
    Bytes(ByteExpr),
    Hash { hash: String },
}

impl Expectation {
    fn resolve(&self, base: &Path) -> anyhow::Result<ResolvedExpectation> {
        match self {
            Self::Bytes(value) => Ok(ResolvedExpectation::Bytes(Base64Bytes(
                value.resolve(base)?,
            ))),
            Self::Hash { hash } => {
                let hash = Sha256Digest::deserialize(serde_json::Value::String(hash.clone()))
                    .map_err(|error| anyhow::anyhow!("decode expectation hash: {error}"))?;
                Ok(ResolvedExpectation::Hash(hash))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedExpectation {
    Bytes(Base64Bytes),
    Hash(Sha256Digest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPatch {
    pub operations: Vec<ResolvedOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedOperation {
    Expect {
        key: Base64Bytes,
        expected: ResolvedExpectation,
    },
    Set {
        key: Base64Bytes,
        value: Base64Bytes,
        expected: Option<ResolvedExpectation>,
    },
    Remove {
        key: Base64Bytes,
        expected: Option<ResolvedExpectation>,
    },
    RemovePrefix {
        prefix: Base64Bytes,
    },
}
