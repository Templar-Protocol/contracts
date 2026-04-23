use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{macros::read_method_spec, OperationId, OperationRecord};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetParams {
    pub operation_id: OperationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetResult {
    pub operation: Option<OperationRecord>,
}

read_method_spec!(
    /// Look up a previously submitted operation by ID.
    "op.get": Get(GetParams) -> GetResult
);
