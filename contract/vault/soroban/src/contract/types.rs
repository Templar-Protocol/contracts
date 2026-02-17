use crate::effects::EffectSummary;

#[derive(Clone)]
pub struct Delta {
    pub market: u32,
    pub amount: u128,
}

#[derive(Clone)]
pub enum AllocationDelta {
    Supply(Delta),
    Withdraw(Delta),
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
