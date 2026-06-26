use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::vault::{
    AllocationDelta, CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey, Fees, MarketId,
    RealAssetsReport, Restrictions, TimelockKind, VaultConfiguration,
};
use templar_common::SU64;
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::NearToken;
use templar_primitives::SU128;

/// Get vault configuration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getConfiguration", output = VaultConfiguration)]
pub struct GetConfiguration {
    pub vault_id: AccountId,
}

/// Get the vault's current total assets.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getTotalAssets", output = SU128)]
pub struct GetTotalAssets {
    pub vault_id: AccountId,
}

/// Get the vault's last fee-anchor total assets.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getLastTotalAssets", output = SU128)]
pub struct GetLastTotalAssets {
    pub vault_id: AccountId,
}

/// Get the vault's idle underlying balance.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getIdleBalance", output = SU128)]
pub struct GetIdleBalance {
    pub vault_id: AccountId,
}

/// Get total share supply.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getTotalSupply", output = SU128)]
pub struct GetTotalSupply {
    pub vault_id: AccountId,
}

/// Get max deposit estimate.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getMaxDeposit", output = SU128)]
pub struct GetMaxDeposit {
    pub vault_id: AccountId,
}

/// Get max single-market deposit estimate.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getMaxSingleMarketDeposit", output = SU128)]
pub struct GetMaxSingleMarketDeposit {
    pub vault_id: AccountId,
}

/// Convert assets to shares.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.convertToShares", output = SU128)]
pub struct ConvertToShares {
    pub vault_id: AccountId,
    pub assets: SU128,
}

/// Convert shares to assets.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.convertToAssets", output = SU128)]
pub struct ConvertToAssets {
    pub vault_id: AccountId,
    pub shares: SU128,
}

/// Preview deposit shares.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.previewDeposit", output = SU128)]
pub struct PreviewDeposit {
    pub vault_id: AccountId,
    pub assets: SU128,
}

/// Preview mint assets.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.previewMint", output = SU128)]
pub struct PreviewMint {
    pub vault_id: AccountId,
    pub shares: SU128,
}

/// Preview withdraw shares.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.previewWithdraw", output = SU128)]
pub struct PreviewWithdraw {
    pub vault_id: AccountId,
    pub assets: SU128,
}

/// Preview redeem assets.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.previewRedeem", output = SU128)]
pub struct PreviewRedeem {
    pub vault_id: AccountId,
    pub shares: SU128,
}

/// Get configured cap groups.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getCapGroups", output = GetCapGroupsResult)]
pub struct GetCapGroups {
    pub vault_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetCapGroupsResult {
    pub cap_groups: Vec<(CapGroupId, CapGroupRecord)>,
}

/// Get fee anchor timestamp.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getFeeAnchorTimestamp", output = SU64)]
pub struct GetFeeAnchorTimestamp {
    pub vault_id: AccountId,
}

/// Get vault fees.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getFees", output = Fees<SU128>)]
pub struct GetFees {
    pub vault_id: AccountId,
}

/// Get current restrictions.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getRestrictions", output = GetRestrictionsResult)]
pub struct GetRestrictions {
    pub vault_id: AccountId,
}

// `Restrictions` does not implement `Debug`, so this result cannot derive it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetRestrictionsResult {
    pub restrictions: Option<Restrictions>,
}

/// Get current withdrawing operation id.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getWithdrawingOpId", output = GetWithdrawingOpIdResult)]
pub struct GetWithdrawingOpId {
    pub vault_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetWithdrawingOpIdResult {
    pub op_id: Option<SU64>,
}

/// Get current withdrawal request id.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getCurrentWithdrawRequestId", output = GetCurrentWithdrawRequestIdResult)]
pub struct GetCurrentWithdrawRequestId {
    pub vault_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetCurrentWithdrawRequestIdResult {
    pub request_id: Option<SU64>,
}

/// Check whether a market withdrawal is pending.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.hasPendingMarketWithdrawal", output = bool)]
pub struct HasPendingMarketWithdrawal {
    pub vault_id: AccountId,
}

/// Get withdrawal queue tail id.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.queueTail", output = SU64)]
pub struct QueueTail {
    pub vault_id: AccountId,
}

/// Peek withdrawal queue head id.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.peekNextPendingWithdrawalId", output = PeekNextPendingWithdrawalIdResult)]
pub struct PeekNextPendingWithdrawalId {
    pub vault_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PeekNextPendingWithdrawalIdResult {
    pub request_id: Option<SU64>,
}

/// Build a real-assets report from stored state.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.buildRealAssetsReport", output = RealAssetsReport)]
pub struct BuildRealAssetsReport {
    pub vault_id: AccountId,
}

/// Get a market id from a market account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getMarketIdOfAccount", output = GetMarketIdOfAccountResult)]
pub struct GetMarketIdOfAccount {
    pub vault_id: AccountId,
    pub market: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetMarketIdOfAccountResult {
    pub market_id: Option<MarketId>,
}

/// Get a market account from a market id.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.getMarketAccountById", output = GetMarketAccountByIdResult)]
pub struct GetMarketAccountById {
    pub vault_id: AccountId,
    pub market_id: SU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetMarketAccountByIdResult {
    pub account_id: Option<AccountId>,
}

/// List configured market ids and accounts.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "vault.listMarketsWithIds", output = ListMarketsWithIdsResult)]
pub struct ListMarketsWithIds {
    pub vault_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListMarketsWithIdsResult {
    pub markets: Vec<(SU64, AccountId)>,
}

/// Deposit underlying into a vault.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.deposit")]
pub struct Deposit {
    pub vault_id: AccountId,
    pub amount: SU128,
}

/// Allocate or withdraw principal from one market.
// `AllocationDelta` does not implement `PartialEq`/`Eq`.
#[derive(MethodSpec, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.allocate")]
pub struct Allocate {
    pub vault_id: AccountId,
    pub delta: AllocationDelta,
}

/// Withdraw underlying by asset amount.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.withdraw")]
pub struct Withdraw {
    pub vault_id: AccountId,
    pub amount: SU128,
    pub receiver: AccountId,
}

/// Redeem shares for underlying.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.redeem")]
pub struct Redeem {
    pub vault_id: AccountId,
    pub shares: SU128,
    pub receiver: AccountId,
}

/// Execute the next withdrawal request.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.executeWithdrawal")]
pub struct ExecuteWithdrawal {
    pub vault_id: AccountId,
    pub route: Vec<MarketId>,
}

/// Execute a market withdrawal step.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.executeMarketWithdrawal")]
pub struct ExecuteMarketWithdrawal {
    pub vault_id: AccountId,
    pub op_id: SU64,
    pub market: MarketId,
    pub batch_limit: Option<u32>,
}

/// Execute an allocator rebalance withdrawal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.executeRebalanceWithdrawal")]
pub struct ExecuteRebalanceWithdrawal {
    pub vault_id: AccountId,
    pub market_id: MarketId,
    pub batch_limit: Option<u32>,
}

/// Resync the vault's idle balance from underlying token balance.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.resyncIdleBalance")]
pub struct ResyncIdleBalance {
    pub vault_id: AccountId,
}

/// Refresh market principal records.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.refreshMarkets")]
pub struct RefreshMarkets {
    pub vault_id: AccountId,
    pub markets: Vec<MarketId>,
}

/// Recover a stuck operation.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.unbrick")]
pub struct Unbrick {
    pub vault_id: AccountId,
}

/// Skim a non-underlying, non-share token from the vault.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.skim")]
pub struct Skim {
    pub vault_id: AccountId,
    pub token: AccountId,
}

/// Accrue vault fees.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.accrueFee")]
pub struct AccrueFee {
    pub vault_id: AccountId,
}

/// Set the supply queue.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.setSupplyQueue")]
pub struct SetSupplyQueue {
    pub vault_id: AccountId,
    pub markets: Vec<MarketId>,
}

/// Submit a market cap change.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.submitCap")]
pub struct SubmitCap {
    pub vault_id: AccountId,
    pub market: AccountId,
    pub new_cap: SU128,
}

/// Accept a market cap change.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.acceptCap")]
pub struct AcceptCap {
    pub vault_id: AccountId,
    pub market: AccountId,
}

/// Revoke a pending market cap change.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.revokePendingCap")]
pub struct RevokePendingCap {
    pub vault_id: AccountId,
    pub market: AccountId,
}

/// Submit a cap-group update.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.submitCapGroupUpdate")]
pub struct SubmitCapGroupUpdate {
    pub vault_id: AccountId,
    pub update: CapGroupUpdate,
}

/// Accept a cap-group update.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.acceptCapGroupUpdate")]
pub struct AcceptCapGroupUpdate {
    pub vault_id: AccountId,
    pub update: CapGroupUpdateKey,
}

/// Revoke a cap-group update.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.revokePendingCapGroupUpdate")]
pub struct RevokePendingCapGroupUpdate {
    pub vault_id: AccountId,
    pub update: CapGroupUpdateKey,
}

/// Submit market removal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.submitMarketRemoval")]
pub struct SubmitMarketRemoval {
    pub vault_id: AccountId,
    pub market: AccountId,
}

/// Accept market removal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.acceptMarketRemoval")]
pub struct AcceptMarketRemoval {
    pub vault_id: AccountId,
    pub market: AccountId,
}

/// Revoke market removal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.revokePendingMarketRemoval")]
pub struct RevokePendingMarketRemoval {
    pub vault_id: AccountId,
    pub market: AccountId,
}

/// Set curator role.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.setCurator")]
pub struct SetCurator {
    pub vault_id: AccountId,
    pub account: AccountId,
}

/// Set allocator role.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.setIsAllocator")]
pub struct SetIsAllocator {
    pub vault_id: AccountId,
    pub account: AccountId,
    pub allowed: bool,
}

/// Submit sentinel role change.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.submitSentinel")]
pub struct SubmitSentinel {
    pub vault_id: AccountId,
    pub account: AccountId,
}

/// Accept sentinel role change.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.acceptSentinel")]
pub struct AcceptSentinel {
    pub vault_id: AccountId,
}

/// Revoke sentinel role change.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.revokePendingSentinel")]
pub struct RevokePendingSentinel {
    pub vault_id: AccountId,
}

/// Set skim recipient.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.setSkimRecipient")]
pub struct SetSkimRecipient {
    pub vault_id: AccountId,
    pub account: AccountId,
}

/// Set fee configuration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.setFees")]
pub struct SetFees {
    pub vault_id: AccountId,
    pub fees: Fees<SU128>,
}

/// Accept fee configuration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.acceptFees")]
pub struct AcceptFees {
    pub vault_id: AccountId,
}

/// Revoke pending fee configuration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.revokePendingFees")]
pub struct RevokePendingFees {
    pub vault_id: AccountId,
}

/// Set restrictions.
// `Restrictions` does not implement `Debug`.
#[derive(MethodSpec, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.setRestrictions")]
pub struct SetRestrictions {
    pub vault_id: AccountId,
    pub restrictions: Option<Restrictions>,
}

/// Accept restrictions.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.acceptRestrictions")]
pub struct AcceptRestrictions {
    pub vault_id: AccountId,
}

/// Revoke pending restrictions.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.revokePendingRestrictions")]
pub struct RevokePendingRestrictions {
    pub vault_id: AccountId,
}

/// Submit timelock configuration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.submitTimelock")]
pub struct SubmitTimelock {
    pub vault_id: AccountId,
    pub new_timelock_ns: SU64,
    pub kind: Option<TimelockKind>,
}

/// Accept timelock configuration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.acceptTimelock")]
pub struct AcceptTimelock {
    pub vault_id: AccountId,
}

/// Revoke pending timelock configuration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "vault.revokePendingTimelock")]
pub struct RevokePendingTimelock {
    pub vault_id: AccountId,
}

/// Fixed deposit charged by the vault for withdrawal queue storage.
pub const WITHDRAW_REQUEST_DEPOSIT: NearToken =
    NearToken::from_yoctonear(2_560_000_000_000_000_000_000);

/// Fixed deposit charged by the vault for supply queue storage additions.
pub const SET_SUPPLY_QUEUE_DEPOSIT: NearToken =
    NearToken::from_yoctonear(840_000_000_000_000_000_000);
