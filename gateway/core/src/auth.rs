use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{primitive::ManagedAccountId, ContractMethodName, WriteMethod};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct PrincipalId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrincipalPolicy {
    pub principal_id: PrincipalId,
    pub allowed_signer_accounts: BTreeSet<ManagedAccountId>,
    pub allowed_write_methods: BTreeSet<AllowedWriteMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub enum AllowedWriteMethod {
    Typed(WriteMethod),
    GenericFunctionCall(GenericCallConstraint),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct GenericCallConstraint {
    pub allowed_receivers: BTreeSet<String>,
    pub allowed_contract_methods: BTreeSet<ContractMethodName>,
}
