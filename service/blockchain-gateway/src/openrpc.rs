use blockchain_gateway_core::RpcMethodMeta;
use schemars::{schema::RootSchema, schema_for};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Document {
    pub openrpc: String,
    pub info: Info,
    pub methods: Vec<Method>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Info {
    pub title: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Method {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "paramStructure")]
    pub param_structure: String,
    pub params: Vec<ContentDescriptor>,
    pub result: ContentDescriptor,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
    pub deprecated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentDescriptor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schema: serde_json::Value,
    pub required: bool,
}

pub fn discover(methods: Vec<Method>) -> Document {
    Document {
        openrpc: "1.4.2".to_owned(),
        info: Info {
            title: "Blockchain Gateway RPC".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: Some(
                "Typed JSON-RPC interface for blockchain gateway operations.".to_owned(),
            ),
        },
        methods,
    }
}

pub fn method<Spec: RpcMethodMeta>() -> Method {
    let input_schema = schema_for!(Spec::Input);
    let output_schema = schema_for!(Spec::Output);

    Method {
        name: Spec::RPC_METHOD.to_owned(),
        summary: non_empty(Spec::SUMMARY).or_else(|| Some(Spec::RPC_METHOD.to_owned())),
        description: non_empty(Spec::DESCRIPTION),
        param_structure: "by-name".to_owned(),
        params: params_from_schema(&input_schema),
        result: ContentDescriptor {
            name: "result".to_owned(),
            summary: None,
            description: description_from_schema(&output_schema),
            schema: serde_json::to_value(&output_schema.schema)
                .expect("schema serialization should succeed"),
            required: true,
        },
        tags: tags_for_method(Spec::RPC_METHOD),
        deprecated: Spec::DEPRECATED,
    }
}

pub fn discover_method() -> Method {
    Method {
        name: "rpc.discover".to_owned(),
        summary: Some("Discover the RPC schema.".to_owned()),
        description: Some(
            "Returns an OpenRPC document describing the blockchain gateway service.".to_owned(),
        ),
        param_structure: "by-name".to_owned(),
        params: vec![],
        result: ContentDescriptor {
            name: "openrpc".to_owned(),
            summary: None,
            description: Some("OpenRPC document for this service.".to_owned()),
            schema: serde_json::json!({
                "type": "object",
                "required": ["openrpc", "info", "methods"],
            }),
            required: true,
        },
        tags: vec![Tag {
            name: "rpc".to_owned(),
        }],
        deprecated: false,
    }
}

fn params_from_schema(schema: &RootSchema) -> Vec<ContentDescriptor> {
    let Some(object) = schema.schema.object.as_ref() else {
        return vec![ContentDescriptor {
            name: "params".to_owned(),
            summary: None,
            description: description_from_schema(schema),
            schema: serde_json::to_value(&schema.schema)
                .expect("schema serialization should succeed"),
            required: true,
        }];
    };

    object
        .properties
        .iter()
        .map(|(name, schema)| ContentDescriptor {
            name: name.clone(),
            summary: None,
            description: schema_description(schema),
            schema: serde_json::to_value(schema).expect("schema serialization should succeed"),
            required: object.required.contains(name),
        })
        .collect()
}

fn tags_for_method(method: &str) -> Vec<Tag> {
    method
        .split('.')
        .next()
        .map(|name| {
            vec![Tag {
                name: name.to_owned(),
            }]
        })
        .unwrap_or_default()
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn description_from_schema(schema: &RootSchema) -> Option<String> {
    schema
        .schema
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.description.clone())
}

fn schema_description(schema: &schemars::schema::Schema) -> Option<String> {
    match schema {
        schemars::schema::Schema::Object(object) => object
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.description.clone()),
        schemars::schema::Schema::Bool(_) => None,
    }
}
