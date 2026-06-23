use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Read,
    Write,
}

/// A gateway method. The implementing type *is* the method input — reads
/// dispatch on it directly, writes wrap it in a [`WriteRequest`].
///
/// [`WriteRequest`]: crate::common::WriteRequest
pub trait MethodSpec: Serialize + DeserializeOwned + JsonSchema + Clone + Send + 'static {
    type Output: Serialize + JsonSchema + Clone + Send + 'static;

    const RPC_METHOD: &'static str;
}

pub trait RpcMethodMeta: MethodSpec {
    const KIND: MethodKind;
    const SUMMARY: &'static str;
    const DESCRIPTION: &'static str;
    const DEPRECATED: bool;
}
