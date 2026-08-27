use std::path::PathBuf;

use near_account_id::AccountId;
use sha2::{Digest as _, Sha256};

use crate::spec::{
    patch::{BorshExpr, ByteExpr, PatchSpec, ResolvedExpectation, ResolvedOperation, Sha256Digest},
    patch_plan::RestoreCode,
};
use templar_gateway_types::CryptoHash;

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("fixtures/spec/patch/{path}"))
}

#[test]
fn byte_expressions_compose_near_collection_keys() {
    let account: AccountId = "alice.near".parse().expect("valid account");
    let expression = ByteExpr::Concat(vec![
        ByteExpr::Hex("00".to_owned()),
        ByteExpr::Sha256(Box::new(ByteExpr::Borsh(BorshExpr {
            type_name: "AccountId".to_owned(),
            value: serde_json::Value::String(account.to_string()),
        }))),
    ]);

    let bytes = expression
        .resolve(std::path::Path::new("."))
        .expect("expression resolves");
    assert_eq!(bytes[0], 0);
    let expected = Sha256::digest(borsh::to_vec(&account).unwrap());
    assert_eq!(&bytes[1..], &expected[..]);
}

#[test]
fn borsh_codecs_are_production_type_encodings() {
    let expression = ByteExpr::Borsh(BorshExpr {
        type_name: "u32".to_owned(),
        value: serde_json::Value::from(7),
    });
    assert_eq!(
        expression.resolve(std::path::Path::new(".")).unwrap(),
        borsh::to_vec(&7u32).unwrap(),
    );

    let account: AccountId = "alice.near".parse().unwrap();
    let cases = [
        (
            "AccountId",
            serde_json::Value::String(account.to_string()),
            borsh::to_vec(&account).unwrap(),
        ),
        (
            "String",
            serde_json::Value::String("hello".to_owned()),
            borsh::to_vec(&"hello".to_owned()).unwrap(),
        ),
        (
            "Vec<u8>",
            serde_json::from_str("[1,2,3]").unwrap(),
            borsh::to_vec(&vec![1_u8, 2, 3]).unwrap(),
        ),
        (
            "bool",
            serde_json::Value::Bool(true),
            borsh::to_vec(&true).unwrap(),
        ),
        (
            "u128",
            serde_json::Value::from(7_u64),
            borsh::to_vec(&7_u128).unwrap(),
        ),
        (
            "u32",
            serde_json::Value::from(7),
            borsh::to_vec(&7_u32).unwrap(),
        ),
        (
            "u64",
            serde_json::Value::from(7_u64),
            borsh::to_vec(&7_u64).unwrap(),
        ),
        (
            "u8",
            serde_json::Value::from(7),
            borsh::to_vec(&7_u8).unwrap(),
        ),
    ];
    for (type_name, value, expected) in cases {
        assert_eq!(
            ByteExpr::Borsh(BorshExpr {
                type_name: type_name.to_owned(),
                value,
            })
            .resolve(std::path::Path::new("."))
            .unwrap(),
            expected
        );
    }
    let unsupported = ByteExpr::Borsh(BorshExpr {
        type_name: "Decimal".to_owned(),
        value: serde_json::Value::String("1.2".to_owned()),
    });
    assert!(unsupported.resolve(std::path::Path::new(".")).is_err());
}

#[test]
fn checked_in_patch_fixture_covers_supported_toml_syntax() {
    let path = fixture("patches/target.near/2026-08-25-syntax.toml");
    let spec = PatchSpec::load(&path).expect("reference spec resolves");
    let resolved = spec.resolve(&path).expect("reference bytes resolve");

    assert_eq!(spec.account_id.as_str(), "target.near");
    assert_eq!(
        resolved
            .operations
            .iter()
            .map(|operation| match operation {
                ResolvedOperation::Expect { .. } => "expect",
                ResolvedOperation::Set { .. } => "set",
                ResolvedOperation::Remove { .. } => "remove",
                ResolvedOperation::RemovePrefix { .. } => "remove_prefix",
            })
            .collect::<Vec<_>>(),
        ["expect", "set", "remove", "remove_prefix", "set", "set"]
    );
    let ResolvedOperation::Set { value, .. } = &resolved.operations[1] else {
        panic!("reference file operation must be a set");
    };
    assert_eq!(value.0, b"reference-position-bytes");
    let ResolvedOperation::Set { value, .. } = &resolved.operations[4] else {
        panic!("reference JSON operation must be a set");
    };
    let expected: serde_json::Value =
        serde_json::from_str(r#"{"enabled":true,"name":"example"}"#).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&value.0).unwrap(),
        expected
    );
    assert!(serde_json::to_value(&resolved)
        .unwrap()
        .to_string()
        .contains(r#""value":"cmVmZXJlbmNlLXBvc2l0aW9uLWJ5dGVz""#));

    let error = PatchSpec::load(&fixture("invalid/unknown-field.toml"))
        .expect_err("unknown top-level key must fail");
    assert!(format!("{error:#}").contains("unknown"));
    let error = PatchSpec::load(&fixture("invalid/tagged-expect.toml"))
        .expect_err("remove_prefix expectation must fail");
    assert!(format!("{error:#}").contains("expect"));

    let ResolvedOperation::Set { expected, .. } = &resolved.operations[5] else {
        panic!("fresh-key operation must be a set");
    };
    assert!(matches!(expected, Some(ResolvedExpectation::Absent)));
}

#[test]
fn sha256_digest_uses_lowercase_hex() {
    let digest = Sha256Digest([0xab; 32]);
    let json = serde_json::to_string(&digest).unwrap();
    assert_eq!(
        json,
        r#""abababababababababababababababababababababababababababababababab""#
    );
    assert_eq!(serde_json::from_str::<Sha256Digest>(&json).unwrap(), digest);
    assert!(serde_json::from_str::<Sha256Digest>(
        r#""ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB""#
    )
    .is_err());
}

#[test]
fn restore_code_variants_round_trip_as_tagged_json() {
    let account_id: AccountId = "code.near".parse().unwrap();
    let hash = CryptoHash::from(near_api::types::CryptoHash([7; 32]));
    let cases = [
        (
            RestoreCode::Local { code_hash: hash },
            r#"{"mode":"local","code_hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"}"#,
        ),
        (
            RestoreCode::GlobalCodeHash { hash },
            r#"{"mode":"global_code_hash","hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"}"#,
        ),
        (
            RestoreCode::GlobalAccount { account_id },
            r#"{"mode":"global_account","account_id":"code.near"}"#,
        ),
    ];
    for (restore, expected) in cases {
        let encoded = serde_json::to_value(&restore).unwrap();
        assert_eq!(
            encoded,
            serde_json::from_str::<serde_json::Value>(expected).unwrap()
        );
        assert_eq!(
            serde_json::from_value::<RestoreCode>(encoded).unwrap(),
            restore
        );
    }
}
