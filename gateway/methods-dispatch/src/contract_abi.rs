use std::io::{Cursor, Read};

use near_abi::{AbiFunction, AbiFunctionKind, AbiFunctionModifier, AbiParameters, AbiRoot};
use serde_json::{Map, Value};
use templar_gateway_core::{GatewayError, GatewayResult};
use wasmparser::{Parser, Payload};

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const MAX_ABI_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) async fn validate_constructor_args(
    wasm: Vec<u8>,
    init_args: Vec<u8>,
) -> GatewayResult<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        validate_constructor_args_sync(&wasm, &init_args)?;
        Ok(init_args)
    })
    .await
    .map_err(|error| GatewayError::Internal(format!("ABI validation task failed: {error}")))?
}

fn validate_constructor_args_sync(wasm: &[u8], init_args: &[u8]) -> GatewayResult<()> {
    let instance = if init_args.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice(init_args).map_err(|error| {
            abi_error(format!(
                "registry deployment init args are not JSON: {error}"
            ))
        })?
    };
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

const MAX_ABI_CANDIDATES: usize = 16;

fn extract_abi(wasm: &[u8]) -> GatewayResult<AbiRoot> {
    let mut candidates = 0;
    let mut first_error = None;
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
                candidates += 1;
                if candidates > MAX_ABI_CANDIDATES {
                    return Err(abi_error(format!(
                        "registry deployment WASM contains more than {MAX_ABI_CANDIDATES} embedded ABI candidates"
                    )));
                }

                let decoded = match decompress(&data.data[offset..]) {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        first_error.get_or_insert_with(|| {
                            format!(
                                "registry deployment embedded ABI candidate cannot be decompressed: {error}"
                            )
                        });
                        continue;
                    }
                };
                match serde_json::from_slice::<AbiRoot>(&decoded) {
                    Ok(abi) => return Ok(abi),
                    Err(error) => {
                        first_error.get_or_insert_with(|| {
                            format!(
                                "registry deployment embedded ABI candidate is invalid: {error}"
                            )
                        });
                    }
                }
            }
        }
    }

    match first_error {
        Some(error) => Err(abi_error(error)),
        None => Err(abi_error("registry deployment WASM has no embedded ABI")),
    }
}

fn zstd_offsets(data: &[u8]) -> impl Iterator<Item = usize> + '_ {
    data.windows(ZSTD_MAGIC.len())
        .enumerate()
        .filter_map(|(offset, window)| (window == ZSTD_MAGIC).then_some(offset))
}

fn decompress(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    let zstd_decoder = zstd::stream::read::Decoder::new(Cursor::new(compressed))?.single_frame();
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
    let definitions = serde_json::to_value(&abi.body.root_schema.definitions).map_err(|error| {
        abi_error(format!(
            "registry deployment ABI definitions cannot be encoded as JSON Schema: {error}"
        ))
    })?;

    let mut properties = Map::new();
    let mut required = Vec::new();
    for parameter in args {
        let parameter_schema = serde_json::to_value(&parameter.type_schema).map_err(|error| {
            abi_error(format!(
                "registry deployment constructor ABI cannot be encoded as JSON Schema: {error}"
            ))
        })?;
        if !parameter_is_optional(&parameter_schema, &definitions)? {
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
        ("definitions".to_owned(), definitions),
    ]);
    if !required.is_empty() {
        schema.insert("required".to_owned(), Value::Array(required));
    }

    Ok(Value::Object(schema))
}

fn parameter_is_optional(parameter_schema: &Value, definitions: &Value) -> GatewayResult<bool> {
    if parameter_schema.get("default").is_some() {
        return Ok(true);
    }

    let schema = Value::Object(Map::from_iter([
        (
            "$schema".to_owned(),
            Value::String("http://json-schema.org/draft-07/schema#".to_owned()),
        ),
        ("definitions".to_owned(), definitions.clone()),
        (
            "allOf".to_owned(),
            Value::Array(vec![parameter_schema.clone()]),
        ),
    ]));
    let validator = jsonschema::draft7::new(&schema).map_err(|error| {
        abi_error(format!(
            "registry deployment constructor ABI contains an invalid JSON Schema: {error}"
        ))
    })?;
    Ok(validator.is_valid(&Value::Null))
}

fn abi_error(reason: impl std::fmt::Display) -> GatewayError {
    GatewayError::RequestPreconditionFailed(format!(
        "{reason}; set skip_abi_check=true to bypass ABI validation"
    ))
}

#[cfg(test)]
mod tests {
    use super::{validate_constructor_args_sync, MAX_ABI_CANDIDATES, ZSTD_MAGIC};
    use rstest::rstest;

    fn wasm_with_abi(abi: &str) -> Vec<u8> {
        let compressed = zstd::stream::encode_all(abi.as_bytes(), 0).expect("compress ABI");
        wasm_with_data(&compressed)
    }

    fn wasm_with_data(data: &[u8]) -> Vec<u8> {
        let mut segment = vec![1, 0, 0x41, 0, 0x0b];
        push_uleb(
            &mut segment,
            u32::try_from(data.len()).expect("data fits Wasm segment length"),
        );
        segment.extend(data);

        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        wasm.push(11);
        push_uleb(
            &mut wasm,
            u32::try_from(segment.len()).expect("Wasm section fits Wasm section length"),
        );
        wasm.extend(segment);
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

    fn valid_abi(args: &str) -> String {
        r#"{"schema_version":"0.4.0","metadata":{},"body":{"functions":[{"name":"new","kind":"call","modifiers":["init"],"params":{"serialization_type":"json","args":__ARGS__}}],"root_schema":{"definitions":{}}}}"#
            .replace("__ARGS__", args)
    }

    fn validate(wasm: &[u8], init_args: &[u8]) -> templar_gateway_core::GatewayResult<()> {
        validate_constructor_args_sync(wasm, init_args)
    }

    #[test]
    fn validates_constructor_args_against_embedded_abi() {
        let wasm = wasm_with_abi(&valid_abi(
            r#"[{"name":"owner_id","type_schema":{"type":"string"}}]"#,
        ));

        validate(&wasm, br#"{"owner_id":"owner.near"}"#).expect("valid constructor arguments");
    }

    #[rstest]
    #[case("{}", "missing required constructor argument")]
    #[case(r#"{"owner_id":3}"#, "wrong constructor argument type")]
    #[case(
        r#"{"owner_id":"owner.near","extra":true}"#,
        "unknown constructor argument"
    )]
    fn rejects_invalid_constructor_args(#[case] args: &str, #[case] reason: &str) {
        let wasm = wasm_with_abi(&valid_abi(
            r#"[{"name":"owner_id","type_schema":{"type":"string"}}]"#,
        ));

        let error = validate(&wasm, args.as_bytes()).expect_err(reason);
        assert!(error.to_string().contains("do not match"));
    }

    #[test]
    fn accepts_empty_arguments_for_no_argument_constructor() {
        let wasm = wasm_with_abi(&valid_abi("[]"));

        validate(&wasm, b"").expect("empty arguments are the empty JSON object");
    }
    #[test]
    fn accepts_embedded_abi_before_trailing_segment_data() {
        let mut data =
            zstd::stream::encode_all(valid_abi("[]").as_bytes(), 0).expect("compress ABI");
        data.extend(b"unrelated trailing Wasm data");

        validate(&wasm_with_data(&data), br"{}")
            .expect("trailing data after embedded ABI is ignored");
    }

    #[test]
    fn accepts_defaulted_and_nullable_parameters() {
        let wasm = wasm_with_abi(
            r##"{"schema_version":"0.4.0","metadata":{},"body":{"functions":[{"name":"new","kind":"call","modifiers":["init"],"params":{"serialization_type":"json","args":[{"name":"defaulted","type_schema":{"type":"string","default":"value"}},{"name":"nullable_type","type_schema":{"type":["string","null"]}},{"name":"nullable_ref","type_schema":{"$ref":"#/definitions/Nullable"}}]}}],"root_schema":{"definitions":{"Nullable":{"anyOf":[{"type":"string"},{"type":"null"}]}}}}}"##,
        );

        validate(&wasm, br"{}").expect("all optional parameters may be omitted");
    }

    #[test]
    fn preserves_first_candidate_error_until_a_valid_candidate() {
        let valid = zstd::stream::encode_all(valid_abi("[]").as_bytes(), 0).expect("compress ABI");
        let mut data = ZSTD_MAGIC.to_vec();
        data.extend([0, 1, 2]);
        data.extend(valid);

        validate(&wasm_with_data(&data), br"{}").expect("later valid candidate");
    }

    #[test]
    fn preserves_malformed_candidate_diagnostic() {
        let error = validate(&wasm_with_data(&ZSTD_MAGIC), br"{}")
            .expect_err("malformed compressed candidate");

        assert!(error
            .to_string()
            .contains("embedded ABI candidate cannot be decompressed"));
    }

    #[test]
    fn preserves_invalid_abi_json_diagnostic() {
        let invalid =
            zstd::stream::encode_all(&b"not an ABI"[..], 0).expect("compress invalid ABI");

        let error = validate(&wasm_with_data(&invalid), br"{}").expect_err("invalid ABI JSON");
        assert!(error
            .to_string()
            .contains("embedded ABI candidate is invalid"));
    }

    #[test]
    fn reports_no_abi_when_data_has_no_candidate() {
        let error = validate(&wasm_with_data(b"not compressed"), br"{}").expect_err("no ABI");

        assert!(error.to_string().contains("has no embedded ABI"));
    }

    #[test]
    fn caps_embedded_abi_candidates() {
        let data = ZSTD_MAGIC.repeat(MAX_ABI_CANDIDATES + 1);

        let error = validate(&wasm_with_data(&data), br"{}").expect_err("candidate cap");
        assert!(error
            .to_string()
            .contains("more than 16 embedded ABI candidates"));
    }

    #[test]
    fn rejects_non_initializing_abi() {
        let wasm = wasm_with_abi(
            r#"{"schema_version":"0.4.0","metadata":{},"body":{"functions":[{"name":"new","kind":"call","modifiers":[],"params":{"serialization_type":"json","args":[]}}],"root_schema":{"definitions":{}}}}"#,
        );
        let error = validate(&wasm, br"{}").expect_err("non-initializing constructor");

        assert!(error.to_string().contains("no initializing new method"));
    }

    #[test]
    fn rejects_missing_embedded_abi() {
        let error = validate(b"\0asm\x01\0\0\0", br"{}").expect_err("missing embedded ABI");

        assert!(error.to_string().contains("no embedded ABI"));
    }
}
