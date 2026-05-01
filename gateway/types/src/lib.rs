pub mod contract;
mod macros;

pub mod error;
pub mod operation;
pub mod primitive;
pub mod rpc;
pub mod spec;
pub mod version;

pub use contract::ContractKind;
pub use error::{CoreError, CoreResult};
pub use operation::{
    OperationId, OperationRecord, OperationStatus, StepStatus, TransactionStepRecord,
};
pub use primitive::{
    Base64Bytes, ContractMethodName, CryptoHash, IdempotencyKey, ManagedAccountId, MarketId,
    NearGas, NearToken, RegistryId, UniversalAccountId, U128,
};
pub use rpc::common;
pub use spec::{MethodKind, MethodSpec, RpcMethodMeta};
pub use version::{
    Market, MarketVersion, ParseError as VersionParseError, Registry, RegistryVersion, Version,
};
