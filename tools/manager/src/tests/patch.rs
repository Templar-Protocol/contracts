use std::path::PathBuf;

use near_account_id::AccountId;
use sha2::{Digest as _, Sha256};

use crate::spec::patch::{BorshExpr, ByteExpr, PatchSpec, ResolvedOperation};

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
    assert_eq!(resolved.operations.len(), 5);
    assert!(matches!(
        &resolved.operations[1],
        ResolvedOperation::Set { value, .. } if value == b"reference-position-bytes"
    ));
    assert!(matches!(
        resolved.operations[3],
        ResolvedOperation::RemovePrefix { .. }
    ));
    let ResolvedOperation::Set { value, .. } = &resolved.operations[4] else {
        panic!("reference JSON operation must be a set");
    };
    let expected: serde_json::Value =
        serde_json::from_str(r#"{"enabled":true,"name":"example"}"#).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(value).unwrap(),
        expected
    );

    assert!(PatchSpec::load(&fixture("invalid/unknown-field.toml")).is_err());
}
