use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};

use crate::{PublicReadMethod, WriteMethod};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    PublicRead,
    Write,
}

pub trait MethodSpec {
    type Input: Serialize + DeserializeOwned + JsonSchema;
    type Output: Serialize + JsonSchema;

    const RPC_METHOD: &'static str;
}

pub trait ReadMethodSpec: MethodSpec {
    const IDENTIFIER: PublicReadMethod;
}

pub trait WriteMethodSpec: MethodSpec {
    const IDENTIFIER: WriteMethod;
}
