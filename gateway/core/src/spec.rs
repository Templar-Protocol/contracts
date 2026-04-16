use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};

pub trait MethodSpec {
    type Input: Serialize + DeserializeOwned + JsonSchema + Send + 'static;
    type Output: Serialize + JsonSchema + Clone + Send + 'static;

    const RPC_METHOD: &'static str;
}
