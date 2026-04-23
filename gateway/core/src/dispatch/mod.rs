mod account;
mod contract;
mod ft;
mod lst_oracle;
mod market;
mod mt;
mod oracle;
mod proxy_oracle;
mod proxy_oracle_governance;
mod proxy_oracle_owner;
mod pyth;
mod redstone;
mod ref_finance;
mod registry;
mod storage;
mod token;
mod tx;
mod universal_account;

use crate::operation::{OperationPlan, PlannedTransaction};

pub(crate) fn single_transaction_plan(transaction: PlannedTransaction) -> OperationPlan {
    OperationPlan::single(transaction)
}
