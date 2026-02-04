use near_sdk::near;

use crate::asset::BorrowAssetAmount;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct IncomingDeposit {
    pub activate_at_snapshot_index: u32,
    pub amount_real: BorrowAssetAmount,
    pub amount_virtual: BorrowAssetAmount,
}
