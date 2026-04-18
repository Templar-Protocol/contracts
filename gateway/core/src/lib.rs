mod macros;

pub mod error;
pub mod operation;
pub mod primitive;
pub mod rpc;
pub mod spec;
pub mod version;

pub use error::{CoreError, CoreResult};
pub use operation::{
    OperationId, OperationOutcome, OperationRecord, OperationStatus, StepStatus,
    TransactionStepRecord,
};
pub use primitive::{
    Base64Bytes, ContractMethodName, CryptoHash, IdempotencyKey, ManagedAccountId, MarketId,
    NearGas, NearToken, RegistryId, UniversalAccountId, U128,
};
pub use rpc::{
    account, common, contract, ft, market, oracle, proxy_oracle, proxy_oracle_governance,
    proxy_oracle_owner, registry, storage, tx, universal_account,
};
pub use spec::MethodSpec;
pub use version::{
    Market, MarketVersion, ParseError as VersionParseError, Registry, RegistryVersion, Version,
};
