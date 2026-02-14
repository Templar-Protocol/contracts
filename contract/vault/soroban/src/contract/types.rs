use soroban_sdk::contracttype;

use crate::effects::EffectSummary;

#[contracttype]
#[derive(Clone)]
pub struct Delta {
    pub market: u32,
    pub amount: u128,
}

#[contracttype]
#[derive(Clone)]
pub enum AllocationDelta {
    Supply(Delta),
    Withdraw(Delta),
}

#[contracttype]
#[derive(Clone)]
pub struct SetCapData {
    pub cap_group_id: soroban_sdk::String,
    pub new_cap: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct SetRelativeCapData {
    pub cap_group_id: soroban_sdk::String,
    pub new_relative_cap_wad: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct SetMembershipData {
    pub market_id: u32,
    // Empty string = unassign from any cap group (Soroban contracttype doesn't support Option<String>).
    pub cap_group_id: soroban_sdk::String,
}

#[contracttype]
#[derive(Clone)]
pub enum CapGroupUpdateSdk {
    SetCap(SetCapData),
    SetRelativeCap(SetRelativeCapData),
    SetMembership(SetMembershipData),
}

#[contracttype]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Eq, PartialEq)]
pub struct VaultSnapshot {
    pub total_shares: i128,
    pub idle_assets: i128,
    pub external_assets: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct VaultAddresses {
    pub curator: soroban_sdk::Address,
    pub governance: soroban_sdk::Address,
    pub asset_token: soroban_sdk::Address,
    pub share_token: soroban_sdk::Address,
}

#[contracttype]
#[derive(Clone)]
pub struct FeeInfo {
    pub anchor_total_assets: i128,
    pub anchor_timestamp_ns: u64,
    pub management_fee_wad: i128,
    pub performance_fee_wad: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct WithdrawStatus {
    pub next_pending_id: i64,
    pub withdrawing_op_id: i64,
    pub current_request_id: i64,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct DepositResult {
    pub shares_minted: u128,
    pub total_shares: u128,
    pub total_assets: u128,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawRequestResult {
    pub request_id: u64,
    pub shares_escrowed: u128,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct AllocationResult {
    pub op_id: u64,
    pub new_external_assets: u128,
    pub summary: EffectSummary,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshResult {
    pub op_id: u64,
    pub markets_refreshed: u32,
    pub new_external_assets: u128,
}
