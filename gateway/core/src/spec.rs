use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};

use crate::method::MethodSelector;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    PublicRead,
    Write,
}

pub trait MethodSpec {
    type Input: Serialize + DeserializeOwned + JsonSchema;
    type Output: Serialize + JsonSchema;

    const RPC_METHOD: &'static str;
    const IDENTIFIER: MethodSelector;
}
