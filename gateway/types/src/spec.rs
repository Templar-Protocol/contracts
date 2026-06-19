use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Read,
    Write,
}

pub trait MethodSpec {
    type Input: Serialize + DeserializeOwned + JsonSchema + Clone + Send + 'static;
    type Output: Serialize + JsonSchema + Clone + Send + 'static;

    const RPC_METHOD: &'static str;
}

pub trait RpcMethodMeta: MethodSpec {
    const KIND: MethodKind;
    const SUMMARY: &'static str;
    const DESCRIPTION: &'static str;
    const DEPRECATED: bool;
}
