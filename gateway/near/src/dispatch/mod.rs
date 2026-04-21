mod account;
mod contract;
mod ft;
mod market;
mod oracle;
mod proxy_oracle;
mod proxy_oracle_governance;
mod proxy_oracle_owner;
mod registry;
mod storage;
mod tx;
mod universal_account;

use crate::operation::{OperationPlan, PlannedTransaction};

pub(crate) fn single_transaction_plan(transaction: PlannedTransaction) -> OperationPlan {
    OperationPlan::single(transaction)
}
