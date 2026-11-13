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
use templar_gateway_types::{Base64Bytes, CryptoHash};

pub const PATCH_SCHEMA_VERSION: u32 = 3;

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
        let mut spec: Self = super::extends::load_raw(
            path,
            PATCH_SCHEMA_VERSION,
            "tmplrmgr patch export <target-account> --out <new-schema-3-path>",
            "Schema 3 adds optional view checks; schema-2 storage checks and operations retain \
             their meaning. Review the authored patch, set `schema = 3`, then rerun \
             `tmplrmgr patch plan`.",
        )?;
        spec.extends.clear();
        spec.validate_view_checks()?;
        Ok(spec)
    }
    pub fn view_checks(&self) -> impl Iterator<Item = &PatchViewCheck> {
        self.checks.iter().filter_map(|check| match check {
            PatchCheck::View(check) => Some(check.as_ref()),
            PatchCheck::Storage(_) | PatchCheck::Code(_) => None,
        })
    }

    pub fn validate_view_checks(&self) -> anyhow::Result<()> {
        for check in self.view_checks() {
            check.expected()?;
        }
        Ok(())
    }

    pub fn verify_code_hash(&self, actual: &CryptoHash) -> anyhow::Result<()> {
        for check in &self.checks {
            if let PatchCheck::Code(check) = check {
                anyhow::ensure!(
                    check.code_hash == *actual,
                    "target code does not match the patch spec: expected {}, got {actual}",
                    check.code_hash
                );
            }
        }
        Ok(())
    }

    pub fn resolve(&self, source_path: &Path) -> anyhow::Result<ResolvedPatch> {
        self.validate_view_checks()?;
        let base = source_path.parent().unwrap_or_else(|| Path::new("."));
        let mut ops = Vec::with_capacity(self.checks.len() + self.ops.len());
        for check in &self.checks {
            if let PatchCheck::Storage(check) = check {
                ops.push(ResolvedOperation::Expect {
                    key: Base64Bytes(check.key.resolve(base)?),
                    expected: check.expect.resolve(base)?,
                });
            }
        }
        for operation in &self.ops {
            ops.push(operation.resolve(base)?);
        }
        Ok(ResolvedPatch { operations: ops })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PatchCheck {
    Storage(PatchStorageCheck),
    View(Box<PatchViewCheck>),
    Code(PatchCodeCheck),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchStorageCheck {
    pub key: ByteExpr,
    pub expect: Expectation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchCodeCheck {
    pub code_hash: CryptoHash,
}
fn default_json_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchViewCheck {
    pub method_name: templar_gateway_types::ContractMethodName,
    #[serde(default = "default_json_object")]
    pub args: serde_json::Value,
    #[serde(default)]
    pub expect: Option<serde_json::Value>,
    /// Exact JSON expectation encoded as text, including nulls TOML cannot express.
    /// Mutually exclusive with `expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub expect_json: Option<JsonText>,
}

impl PatchViewCheck {
    pub fn expected(&self) -> anyhow::Result<Option<&serde_json::Value>> {
        anyhow::ensure!(
            self.expect.is_none() || self.expect_json.is_none(),
            "view check cannot specify both expect and expect_json"
        );
        Ok(self
            .expect
            .as_ref()
            .or_else(|| self.expect_json.as_ref().map(|text| &text.value)))
    }
}

/// Validated JSON text kept as a string in TOML and plan JSON. Retaining the
/// original text makes round trips and plan digests independent of reparsing.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonText {
    text: String,
    value: serde_json::Value,
}

impl JsonText {
    pub fn from_value(value: serde_json::Value) -> anyhow::Result<Self> {
        Ok(Self {
            text: serde_json::to_string(&value)?,
            value,
        })
    }
}

impl Serialize for JsonText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
    }
}

impl<'de> Deserialize<'de> for JsonText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let value = serde_json::from_str(&text).map_err(D::Error::custom)?;
        Ok(Self { text, value })
    }
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
    PinnedFile(PinnedFile),
    Concat(Vec<ByteExpr>),
    Sha256(Box<ByteExpr>),
    Json(serde_json::Value),
    Borsh(BorshExpr),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedFile {
    pub path: PathBuf,
    #[schemars(with = "String")]
    pub sha256: Sha256Digest,
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
            Self::PinnedFile(file) => {
                let path = base.join(&file.path);
                let bytes = std::fs::read(&path).with_context(|| {
                    format!("read pinned byte expression file {}", path.display())
                })?;
                anyhow::ensure!(
                    <[u8; 32]>::from(Sha256::digest(&bytes)) == file.sha256.0,
                    "SHA-256 mismatch for pinned patch file {}",
                    path.display()
                );
                Ok(bytes)
            }
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
#[serde(rename_all = "snake_case")]
pub enum AbsentExpectation {
    Absent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Expectation {
    Bytes(ByteExpr),
    Hash { hash: String },
    Absent(AbsentExpectation),
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
            Self::Absent(AbsentExpectation::Absent) => Ok(ResolvedExpectation::Absent),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedExpectation {
    Bytes(Base64Bytes),
    Hash(Sha256Digest),
    Absent,
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

#[cfg(test)]
mod view_expectation_tests {
    use super::*;
    use serde_json::json;

    fn check(expect: Option<serde_json::Value>, expect_json: Option<JsonText>) -> PatchViewCheck {
        PatchViewCheck {
            method_name: "get_configuration".to_owned().into(),
            args: json!({}),
            expect,
            expect_json,
        }
    }

    #[test]
    fn legacy_view_expectation_json_wire_is_unchanged() {
        for (expect, expected_json) in [
            (
                None,
                r#"{"method_name":"get_configuration","args":{},"expect":null}"#,
            ),
            (
                Some(json!({"enabled":true})),
                r#"{"method_name":"get_configuration","args":{},"expect":{"enabled":true}}"#,
            ),
        ] {
            let original = check(expect, None);
            assert_eq!(serde_json::to_string(&original).unwrap(), expected_json);
            let restored: PatchViewCheck = serde_json::from_str(expected_json).unwrap();
            assert_eq!(restored, original);
        }
    }

    #[test]
    fn typed_json_text_preserves_nested_and_root_null_through_toml_and_json() {
        for expected in [
            json!({"maximum":null,"array":[null,{"nested":null}]}),
            json!(null),
        ] {
            let original = check(None, Some(JsonText::from_value(expected.clone()).unwrap()));
            let toml = toml::to_string_pretty(&original).unwrap();
            let loaded: PatchViewCheck = toml::from_str(&toml).unwrap();
            assert_eq!(loaded.expected().unwrap(), Some(&expected));
            let restored: PatchViewCheck =
                serde_json::from_slice(&serde_json::to_vec(&loaded).unwrap()).unwrap();
            assert_eq!(restored, original);
            assert_eq!(restored.expected().unwrap(), Some(&expected));
        }
        let text = " { \"maximum\" : null } ";
        let parsed: JsonText = serde_json::from_value(json!(text)).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(text));
    }

    #[test]
    fn rejects_malformed_text_nonstring_input_and_conflicting_expectations() {
        for invalid in [json!("{"), json!(123), json!({"maximum":null})] {
            assert!(serde_json::from_value::<JsonText>(invalid).is_err());
        }
        let conflict = check(
            Some(json!({"old":true})),
            Some(JsonText::from_value(json!({"new":true})).unwrap()),
        );
        assert!(conflict.expected().is_err());
        let spec = PatchSpec {
            schema: PATCH_SCHEMA_VERSION,
            extends: Vec::new(),
            account_id: "market.near".parse().unwrap(),
            ops: Vec::new(),
            checks: vec![PatchCheck::View(Box::new(conflict))],
        };
        assert!(spec.resolve(Path::new("unused.toml")).is_err());
    }

    #[test]
    fn loader_accepts_nullable_text_and_rejects_conflicts_before_resolution() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tmplrmgr-view-expectation-{}-{unique}",
            std::process::id(),
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("patch.toml");
        let base = "schema = 3\naccount_id = 'market.near'\n[[check]]\nmethod_name = 'get_configuration'\n";
        std::fs::write(
            &path,
            format!("{base}expect_json = '{{\"maximum\":null}}'\n"),
        )
        .unwrap();
        let spec = PatchSpec::load(&path).unwrap();
        assert_eq!(
            spec.view_checks().next().unwrap().expected().unwrap(),
            Some(&json!({"maximum":null})),
        );
        std::fs::write(
            &path,
            format!("{base}expect = {{ enabled = true }}\nexpect_json = '{{\"maximum\":null}}'\n"),
        )
        .unwrap();
        assert!(PatchSpec::load(&path).is_err());
        std::fs::write(&path, format!("{base}expect_json = '{{'\n")).unwrap();
        assert!(PatchSpec::load(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
