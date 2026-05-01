use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::read_method_spec;
use templar_gateway_types::{OperationId, OperationRecord};

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
