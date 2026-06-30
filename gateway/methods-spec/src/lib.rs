pub mod account;
pub mod chain;
pub mod contract;
pub mod ft;
pub mod lst_oracle;
pub mod market;
pub mod mt;
pub mod op;
pub mod oracle;
pub mod proxy_oracle;
pub mod proxy_oracle_governance;
pub mod proxy_oracle_owner;
pub mod pyth;
pub mod redstone;
pub mod ref_finance;
pub mod registry;
pub mod storage;
pub mod token;
pub mod tx;
pub mod universal_account;

/// Invoke `$callback!($spec)` once for every **read** method served by
/// [`templar_gateway_methods_dispatch::Dispatch`].
///
/// **Whenever you add or remove a gateway read method, add or remove its line
/// here.** Together with [`for_each_write_method`] this is the single canonical
/// list of these methods: the RPC service expands it to register handlers and
/// the catalog crate expands it to build `gateway/METHODS.md`, so registration
/// and documentation cannot drift apart. Removing a method's spec without
/// removing its line here is a compile error; adding a spec without adding a line
/// here leaves it unregistered and undocumented.
///
/// Excludes [`op::Get`], which reads the operation store rather than the chain
/// and so is registered specially (see `register_operation_get` in the service);
/// it is the one method handled outside these macros.
#[macro_export]
macro_rules! for_each_read_method {
    ($callback:ident) => {
        $callback!($crate::account::Get);
        $callback!($crate::account::GetAccessKey);
        $callback!($crate::chain::GetBlock);
        $callback!($crate::contract::ViewFunction);
        $callback!($crate::contract::GetKind);
        $callback!($crate::contract::GetVersion);
        $callback!($crate::ft::GetBalanceOf);
        $callback!($crate::lst_oracle::GetOracleId);
        $callback!($crate::lst_oracle::ListTransformers);
        $callback!($crate::lst_oracle::GetTransformer);
        $callback!($crate::market::GetConfiguration);
        $callback!($crate::market::GetCurrentSnapshot);
        $callback!($crate::market::GetFinalizedSnapshotsLen);
        $callback!($crate::market::ListFinalizedSnapshots);
        $callback!($crate::market::GetBorrowAssetMetrics);
        $callback!($crate::market::ListBorrowPositions);
        $callback!($crate::market::GetBorrowPosition);
        $callback!($crate::market::GetBorrowPositionPendingInterest);
        $callback!($crate::market::GetBorrowStatus);
        $callback!($crate::market::ListSupplyPositions);
        $callback!($crate::market::GetSupplyPosition);
        $callback!($crate::market::GetSupplyPositionPendingYield);
        $callback!($crate::market::GetSupplyWithdrawalRequestStatus);
        $callback!($crate::market::GetSupplyWithdrawalQueueStatus);
        $callback!($crate::market::GetLastYieldRate);
        $callback!($crate::market::GetStaticYield);
        $callback!($crate::mt::GetBalanceOf);
        $callback!($crate::mt::GetBatchBalanceOf);
        $callback!($crate::mt::GetSupply);
        $callback!($crate::mt::GetBatchSupply);
        $callback!($crate::proxy_oracle::ListProxies);
        $callback!($crate::proxy_oracle::GetProxy);
        $callback!($crate::proxy_oracle::PriceFeedExists);
        $callback!($crate::proxy_oracle_governance::NextProposalId);
        $callback!($crate::proxy_oracle_governance::ProposalCount);
        $callback!($crate::proxy_oracle_governance::GetOperationTtl);
        $callback!($crate::proxy_oracle_governance::ListProposals);
        $callback!($crate::proxy_oracle_governance::GetProposal);
        $callback!($crate::proxy_oracle_owner::GetOwner);
        $callback!($crate::proxy_oracle_owner::GetProposedOwner);
        $callback!($crate::ref_finance::GetPools);
        $callback!($crate::registry::GetDeployment);
        $callback!($crate::registry::ListDeployments);
        $callback!($crate::registry::ListDeploymentsByKind);
        $callback!($crate::registry::ListVersions);
        $callback!($crate::storage::GetBalanceBounds);
        $callback!($crate::storage::GetBalanceOf);
        $callback!($crate::token::GetBalanceOf);
        $callback!($crate::tx::Get);
        $callback!($crate::universal_account::GetKey);
        $callback!($crate::oracle::GetPriceResolutionDependencies);
        $callback!($crate::oracle::ResolvePrice);
        $callback!($crate::oracle::ResolvePrices);
        $callback!($crate::oracle::GetPrice);
        $callback!($crate::oracle::GetPrices);
        $callback!($crate::pyth::ListEmaPricesNoOlderThan);
        $callback!($crate::pyth::ListEmaPricesUnsafe);
        $callback!($crate::redstone::GetConfig);
        $callback!($crate::redstone::ReadPriceData);
        $callback!($crate::redstone::ListRole);
    };
}

/// Invoke `$callback!($spec)` once for every **write** method served by
/// [`templar_gateway_methods_dispatch::Dispatch`]. Add or remove a line here
/// whenever you add or remove a write method — see [`for_each_read_method`].
#[macro_export]
macro_rules! for_each_write_method {
    ($callback:ident) => {
        $callback!($crate::account::Delete);
        $callback!($crate::ft::Transfer);
        $callback!($crate::ft::TransferCall);
        $callback!($crate::market::Create);
        $callback!($crate::market::Borrow);
        $callback!($crate::market::Supply);
        $callback!($crate::market::WithdrawCollateral);
        $callback!($crate::market::ApplyInterest);
        $callback!($crate::market::Repay);
        $callback!($crate::market::CreateSupplyWithdrawalRequest);
        $callback!($crate::market::CancelSupplyWithdrawalRequest);
        $callback!($crate::market::ExecuteNextSupplyWithdrawalRequest);
        $callback!($crate::market::WithdrawSupply);
        $callback!($crate::market::Liquidate);
        $callback!($crate::market::HarvestYield);
        $callback!($crate::market::AccumulateStaticYield);
        $callback!($crate::market::WithdrawStaticYield);
        $callback!($crate::mt::Transfer);
        $callback!($crate::mt::TransferCall);
        $callback!($crate::proxy_oracle::UpdatePrices);
        $callback!($crate::proxy_oracle_governance::CreateProposal);
        $callback!($crate::proxy_oracle_governance::CancelProposal);
        $callback!($crate::proxy_oracle_governance::ExecuteProposal);
        $callback!($crate::proxy_oracle_owner::ProposeOwner);
        $callback!($crate::proxy_oracle_owner::AcceptOwner);
        $callback!($crate::proxy_oracle_owner::RenounceOwner);
        $callback!($crate::registry::AddVersion);
        $callback!($crate::registry::RemoveVersion);
        $callback!($crate::registry::Deploy);
        $callback!($crate::storage::Deposit);
        $callback!($crate::storage::EnsureDeposit);
        $callback!($crate::storage::Unregister);
        $callback!($crate::token::Transfer);
        $callback!($crate::token::TransferCall);
        $callback!($crate::tx::FunctionCall);
        $callback!($crate::tx::Transfer);
        $callback!($crate::tx::RelaySignedDelegateAction);
        $callback!($crate::tx::DeployContract);
        $callback!($crate::tx::DeployAndInit);
        $callback!($crate::universal_account::Execute);
        $callback!($crate::universal_account::Create);
        $callback!($crate::pyth::UpdatePriceFeeds);
        $callback!($crate::redstone::SetRole);
        $callback!($crate::redstone::WritePrices);
    };
}
