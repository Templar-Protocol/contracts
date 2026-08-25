use std::io::{Cursor, Read};

use near_abi::{AbiFunction, AbiFunctionKind, AbiFunctionModifier, AbiParameters, AbiRoot};
use serde_json::{Map, Value};
use templar_gateway_core::{GatewayError, GatewayResult};
use wasmparser::{Parser, Payload};

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const MAX_ABI_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn validate_constructor_args(wasm: &[u8], init_args: &[u8]) -> GatewayResult<()> {
    let instance = serde_json::from_slice(init_args).map_err(|error| {
        abi_error(format!(
            "registry deployment init args are not JSON: {error}"
        ))
    })?;
    let schema = constructor_schema(wasm)?;
    let validator = jsonschema::draft7::new(&schema).map_err(|error| {
        abi_error(format!(
            "registry deployment constructor ABI contains an invalid JSON Schema: {error}"
        ))
    })?;

    validator.validate(&instance).map_err(|error| {
        abi_error(format!(
            "registry deployment init args do not match the constructor ABI: {error}"
        ))
    })
}

fn constructor_schema(wasm: &[u8]) -> GatewayResult<Value> {
    let abi = extract_abi(wasm)?;
    let constructor = abi
        .body
        .functions
        .iter()
        .find(|function| {
            function.name == "new"
                && matches!(&function.kind, AbiFunctionKind::Call)
                && function.modifiers.contains(&AbiFunctionModifier::Init)
        })
        .ok_or_else(|| abi_error("registry deployment ABI has no initializing new method"))?;

    constructor_json_schema(&abi, constructor)
}

fn extract_abi(wasm: &[u8]) -> GatewayResult<AbiRoot> {
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::DataSection(section) = payload.map_err(|error| {
            abi_error(format!("registry deployment WASM is malformed: {error}"))
        })?
        else {
            continue;
        };

        for data in section {
            let data = data.map_err(|error| {
                abi_error(format!(
                    "registry deployment WASM data section is malformed: {error}"
                ))
            })?;

            for offset in zstd_offsets(data.data) {
                let Ok(decoded) = decompress(&data.data[offset..]) else {
                    continue;
                };
                if let Ok(abi) = serde_json::from_slice(&decoded) {
                    return Ok(abi);
                }
            }
        }
    }

    Err(abi_error("registry deployment WASM has no embedded ABI"))
}

fn zstd_offsets(data: &[u8]) -> impl Iterator<Item = usize> + '_ {
    data.windows(ZSTD_MAGIC.len())
        .enumerate()
        .filter_map(|(offset, window)| (window == ZSTD_MAGIC).then_some(offset))
}

fn decompress(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    let zstd_decoder = zstd::stream::read::Decoder::new(Cursor::new(compressed))?;
    let mut decoded = Vec::new();
    zstd_decoder
        .take(MAX_ABI_BYTES + 1)
        .read_to_end(&mut decoded)?;
    if decoded.len() as u64 > MAX_ABI_BYTES {
        return Err(std::io::Error::other("embedded ABI exceeds maximum size"));
    }
    Ok(decoded)
}

fn constructor_json_schema(abi: &AbiRoot, constructor: &AbiFunction) -> GatewayResult<Value> {
    let AbiParameters::Json { args } = &constructor.params else {
        return Err(abi_error(
            "registry deployment constructor ABI does not use JSON parameters",
        ));
    };

    let mut properties = Map::new();
    let mut required = Vec::new();
    for parameter in args {
        let parameter_schema = serde_json::to_value(&parameter.type_schema).map_err(|error| {
            abi_error(format!(
                "registry deployment constructor ABI cannot be encoded as JSON Schema: {error}"
            ))
        })?;
        if !allows_null(&parameter_schema) {
            required.push(Value::String(parameter.name.clone()));
        }
        properties.insert(parameter.name.clone(), parameter_schema);
    }

    let mut schema = Map::from_iter([
        (
            "$schema".to_owned(),
            Value::String("http://json-schema.org/draft-07/schema#".to_owned()),
        ),
        ("type".to_owned(), Value::String("object".to_owned())),
        ("properties".to_owned(), Value::Object(properties)),
        ("additionalProperties".to_owned(), Value::Bool(false)),
        (
            "definitions".to_owned(),
            serde_json::to_value(&abi.body.root_schema.definitions).map_err(|error| {
                abi_error(format!(
                    "registry deployment ABI definitions cannot be encoded as JSON Schema: {error}"
                ))
            })?,
        ),
    ]);
    if !required.is_empty() {
        schema.insert("required".to_owned(), Value::Array(required));
    }

    Ok(Value::Object(schema))
}

fn allows_null(schema: &Value) -> bool {
    schema.get("nullable").and_then(Value::as_bool) == Some(true)
        || schema.get("type").and_then(Value::as_str) == Some("null")
        || ["anyOf", "oneOf"]
            .into_iter()
            .filter_map(|key| schema.get(key).and_then(Value::as_array))
            .flatten()
            .any(|schema| schema.get("type").and_then(Value::as_str) == Some("null"))
}

fn abi_error(reason: impl std::fmt::Display) -> GatewayError {
    GatewayError::RequestPreconditionFailed(format!(
        "{reason}; pass --skip-abi-check to bypass ABI validation"
    ))
}

#[cfg(test)]
mod tests {
    use super::validate_constructor_args;

    fn wasm_with_abi(abi: &str) -> Vec<u8> {
        let compressed = zstd::stream::encode_all(abi.as_bytes(), 0).expect("compress ABI");
        let mut section = vec![1, 0, 0x41, 0, 0x0b];
        push_uleb(
            &mut section,
            u32::try_from(compressed.len()).expect("compressed ABI fits Wasm section length"),
        );
        section.extend(compressed);

        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        wasm.push(11);
        push_uleb(
            &mut wasm,
            u32::try_from(section.len()).expect("Wasm section fits Wasm section length"),
        );
        wasm.extend(section);
        wasm
    }

    fn push_uleb(bytes: &mut Vec<u8>, mut value: u32) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                return;
            }
        }
    }

    #[test]
    fn validates_constructor_args_against_embedded_abi() {
        let wasm = wasm_with_abi(
            r#"{"schema_version":"0.4.0","metadata":{},"body":{"functions":[{"name":"new","kind":"call","modifiers":["init"],"params":{"serialization_type":"json","args":[{"name":"owner_id","type_schema":{"type":"string"}}]}}],"root_schema":{"definitions":{}}}}"#,
        );

        validate_constructor_args(&wasm, br#"{"owner_id":"owner.near"}"#)
            .expect("valid constructor arguments");
        let error = validate_constructor_args(&wasm, br"{}")
            .expect_err("missing required constructor argument");
        assert!(error.to_string().contains("do not match"));
        let error = validate_constructor_args(&wasm, br#"{"owner_id":3}"#)
            .expect_err("wrong constructor argument type");
        assert!(error.to_string().contains("do not match"));
        let error = validate_constructor_args(&wasm, br#"{"owner_id":"owner.near","extra":true}"#)
            .expect_err("unknown constructor argument");
        assert!(error.to_string().contains("do not match"));
    }

    #[test]
    fn preserves_abi_definitions_and_optional_parameters() {
        let wasm = wasm_with_abi(
            r##"{"schema_version":"0.4.0","metadata":{},"body":{"functions":[{"name":"new","kind":"call","modifiers":["init"],"params":{"serialization_type":"json","args":[{"name":"owner_id","type_schema":{"anyOf":[{"$ref":"#/definitions/Owner"},{"type":"null"}]}}]}}],"root_schema":{"definitions":{"Owner":{"type":"string"}}}}}"##,
        );

        validate_constructor_args(&wasm, br"{}").expect("optional constructor argument");
        let error = validate_constructor_args(&wasm, br#"{"owner_id":7}"#)
            .expect_err("definition-backed constructor argument type");
        assert!(error.to_string().contains("do not match"));
    }

    #[test]
    fn rejects_missing_or_non_initializing_abi() {
        let error = validate_constructor_args(b"\0asm\x01\0\0\0", br"{}")
            .expect_err("missing embedded ABI");
        assert!(error.to_string().contains("no embedded ABI"));

        let wasm = wasm_with_abi(
            r#"{"schema_version":"0.4.0","metadata":{},"body":{"functions":[{"name":"new","kind":"call","modifiers":[],"params":{"serialization_type":"json","args":[]}}],"root_schema":{"definitions":{}}}}"#,
        );
        let error =
            validate_constructor_args(&wasm, br"{}").expect_err("non-initializing constructor");
        assert!(error.to_string().contains("no initializing new method"));
    }
}
