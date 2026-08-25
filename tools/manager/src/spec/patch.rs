use std::path::{Path, PathBuf};

use anyhow::Context as _;
use base64::Engine as _;
use borsh::BorshSerialize;
use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const PATCH_SCHEMA_VERSION: u32 = 1;

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
        let mut spec: Self = super::extends::load_raw(path, PATCH_SCHEMA_VERSION, "patch plan")?;
        spec.extends.clear();
        Ok(spec)
    }

    pub fn resolve(&self, source_path: &Path) -> anyhow::Result<ResolvedPatch> {
        let base = source_path.parent().unwrap_or_else(|| Path::new("."));
        let mut ops = Vec::with_capacity(self.checks.len() + self.ops.len());
        for check in &self.checks {
            ops.push(ResolvedOperation::Expect {
                key: check.key.resolve(base)?,
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
#[serde(untagged)]
pub enum PatchOperation {
    Set(SetOperation),
    Remove(RemoveOperation),
    RemovePrefix(RemovePrefixOperation),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetOperation {
    pub key: ByteExpr,
    pub value: ByteExpr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expectation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveOperation {
    pub remove: ByteExpr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expectation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemovePrefixOperation {
    pub remove_prefix: ByteExpr,
}

impl PatchOperation {
    fn resolve(&self, base: &Path) -> anyhow::Result<ResolvedOperation> {
        match self {
            Self::Set(operation) => Ok(ResolvedOperation::Set {
                key: operation.key.resolve(base)?,
                value: operation.value.resolve(base)?,
                expected: operation
                    .expect
                    .as_ref()
                    .map(|expect| expect.resolve(base))
                    .transpose()?,
            }),
            Self::Remove(operation) => Ok(ResolvedOperation::Remove {
                key: operation.remove.resolve(base)?,
                expected: operation
                    .expect
                    .as_ref()
                    .map(|expect| expect.resolve(base))
                    .transpose()?,
            }),
            Self::RemovePrefix(operation) => Ok(ResolvedOperation::RemovePrefix {
                prefix: operation.remove_prefix.resolve(base)?,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Expectation {
    Bytes(ByteExpr),
    Hash { hash: String },
}

impl Expectation {
    fn resolve(&self, base: &Path) -> anyhow::Result<ResolvedExpectation> {
        match self {
            Self::Bytes(value) => Ok(ResolvedExpectation::Bytes(value.resolve(base)?)),
            Self::Hash { hash } => {
                let digest = hex::decode(hash).context("decode expectation hash")?;
                let hash: [u8; 32] = digest
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("expectation hash must be 32 bytes"))?;
                Ok(ResolvedExpectation::Hash(hash))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedExpectation {
    Bytes(Vec<u8>),
    Hash([u8; 32]),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPatch {
    pub operations: Vec<ResolvedOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedOperation {
    Expect {
        key: Vec<u8>,
        expected: ResolvedExpectation,
    },
    Set {
        key: Vec<u8>,
        value: Vec<u8>,
        expected: Option<ResolvedExpectation>,
    },
    Remove {
        key: Vec<u8>,
        expected: Option<ResolvedExpectation>,
    },
    RemovePrefix {
        prefix: Vec<u8>,
    },
}
