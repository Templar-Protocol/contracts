pub mod block;
pub mod common;
pub mod contract;
mod macros;

pub mod error;
pub mod operation;
pub mod primitive;
pub mod protocol;
pub use protocol::ProtocolLimits;
pub mod spec;
pub mod version;

pub use block::BlockSummary;
pub use common::ProposalEncoding;
pub use contract::{ContractKind, OracleContractKind};
pub use error::{CoreError, CoreResult};
pub use operation::{
    OperationId, OperationRecord, OperationStatus, StepStatus, TransactionStepRecord,
};
pub use primitive::{
    ActionInput, Base64Bytes, ContractMethodName, CryptoHash, GlobalContractIdentifierInput,
    IdempotencyKey, ManagedAccountId, NearGas, NearToken, SignedDelegateActionInput,
};
pub use spec::{MethodKind, MethodSpec, RpcMethodMeta};
pub use version::{
    Market, MarketVersion, ParseError as VersionParseError, ProxyGovernance,
    ProxyGovernanceVersion, ProxyOracle, ProxyOracleVersion, Registry, RegistryVersion, Version,
};
