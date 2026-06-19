pub mod common;
pub mod contract;
mod macros;

pub mod error;
pub mod operation;
pub mod primitive;
pub mod spec;
pub mod version;

pub use contract::ContractKind;
pub use error::{CoreError, CoreResult};
pub use operation::{
    OperationId, OperationRecord, OperationStatus, StepStatus, TransactionStepRecord,
};
pub use primitive::{
    Base64Bytes, ContractMethodName, CryptoHash, IdempotencyKey, ManagedAccountId, NearGas,
    NearToken, U128,
};
pub use spec::{MethodKind, MethodSpec, RpcMethodMeta};
pub use version::{
    Market, MarketVersion, ParseError as VersionParseError, ProxyOracle, ProxyOracleVersion,
    Registry, RegistryVersion, Version,
};
