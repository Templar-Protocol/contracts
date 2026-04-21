use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{macros::public_read_method_spec, OperationId, OperationRecord};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetParams {
    pub operation_id: OperationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetResult {
    pub operation: Option<OperationRecord>,
}

public_read_method_spec!(Get, "op.get", GetParams, GetResult);
