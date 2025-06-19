use near_sdk::{near, AccountId};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    snapshot::Snapshot,
};

#[near(event_json(standard = "templar-market"))]
pub enum MarketEvent {
    #[event_version("1.0.0")]
    SnapshotFinalized {
        index: u32,
        #[serde(flatten)]
        snapshot: Snapshot,
    },
    #[event_version("1.0.0")]
    GlobalYieldDistributed {
        borrow_asset_amount: BorrowAssetAmount,
    },
    #[event_version("1.0.0")]
    YieldAccumulated {
        account_id: AccountId,
        borrow_asset_amount: BorrowAssetAmount,
    },
    #[event_version("1.0.0")]
    InterestAccumulated {
        account_id: AccountId,
        borrow_asset_amount: BorrowAssetAmount,
    },
    #[event_version("1.0.0")]
    SupplyDeposited {
        account_id: AccountId,
        borrow_asset_amount: BorrowAssetAmount,
    },
    #[event_version("1.0.0")]
    SupplyWithdrawn {
        account_id: AccountId,
        borrow_asset_amount_to_account: BorrowAssetAmount,
        borrow_asset_amount_to_fees: BorrowAssetAmount,
    },
    #[event_version("1.0.0")]
    CollateralDeposited {
        account_id: AccountId,
        collateral_asset_amount: CollateralAssetAmount,
    },
    #[event_version("1.0.0")]
    CollateralWithdrawn {
        account_id: AccountId,
        collateral_asset_amount: CollateralAssetAmount,
    },
    #[event_version("1.0.0")]
    BorrowWithdrawn {
        account_id: AccountId,
        borrow_asset_amount: BorrowAssetAmount,
    },
    #[event_version("1.0.0")]
    BorrowRepaid {
        account_id: AccountId,
        borrow_asset_fees_repaid: BorrowAssetAmount,
        borrow_asset_principal_repaid: BorrowAssetAmount,
        borrow_asset_principal_remaining: BorrowAssetAmount,
    },
    #[event_version("1.0.0")]
    FullLiquidation {
        liquidator_id: AccountId,
        account_id: AccountId,
        borrow_asset_principal: BorrowAssetAmount,
        borrow_asset_recovered: BorrowAssetAmount,
        collateral_asset_liquidated: CollateralAssetAmount,
    },
}
