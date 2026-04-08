mod macros;

pub mod error;
pub mod method;
pub mod operation;
pub mod primitive;
pub mod rpc;
pub mod spec;

pub use error::{CoreError, CoreResult};
pub use method::{
    ChainReadMethod, GenericWriteMethod, MarketReadMethod, MarketWriteMethod, PublicReadMethod,
    RegistryReadMethod, RegistryWriteMethod, StorageReadMethod, StorageWriteMethod,
    UniversalAccountReadMethod, UniversalAccountWriteMethod, WriteMethod,
};
pub use operation::{
    OperationId, OperationOutcome, OperationRecord, OperationStatus, StepStatus,
    TransactionStepRecord,
};
pub use primitive::{
    Base64Bytes, ContractMethodName, IdempotencyKey, ManagedAccountId, MarketId, NearGas,
    NearToken, RegistryId, UniversalAccountId,
};
pub use rpc::{chain, common, market, registry, storage, tx, universal_account};
pub use spec::{MethodKind, MethodSpec, ReadMethodSpec, WriteMethodSpec};
