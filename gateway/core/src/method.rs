use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum PublicReadMethod {
    Chain(ChainReadMethod),
    Registry(RegistryReadMethod),
    Market(MarketReadMethod),
    UniversalAccount(UniversalAccountReadMethod),
    Storage(StorageReadMethod),
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum WriteMethod {
    Generic(GenericWriteMethod),
    Registry(RegistryWriteMethod),
    Market(MarketWriteMethod),
    UniversalAccount(UniversalAccountWriteMethod),
    Storage(StorageWriteMethod),
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum ChainReadMethod {
    ViewAccount,
    ViewFunction,
    GetTransaction,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum RegistryReadMethod {
    ListDeployments,
    ListVersions,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum MarketReadMethod {
    GetConfiguration,
    ListBorrowPositions,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum UniversalAccountReadMethod {
    GetKey,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum StorageReadMethod {
    GetBalanceBounds,
    GetBalanceOf,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum GenericWriteMethod {
    FunctionCall,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum RegistryWriteMethod {
    Deploy,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum MarketWriteMethod {
    Borrow,
    Supply,
    WithdrawCollateral,
    Repay,
    Liquidate,
    AccumulateBorrow,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum UniversalAccountWriteMethod {
    Execute,
    CreateAccount,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum StorageWriteMethod {
    Deposit,
    EnsureDeposit,
}
