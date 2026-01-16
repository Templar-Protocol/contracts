use near_api::types::transaction::result::ExecutionSuccess;
use near_sdk::{json_types::U128, AccountId};

use crate::{FtController, StorageManagementController, TestAccount};

use super::{mt::MtController, ContractController};

#[derive(Clone)]
pub enum TokenController {
    Ft {
        controller: FtController,
    },
    Mt {
        controller: MtController,
        token_id: String,
    },
}

impl ContractController for TokenController {
    fn account(&self) -> &TestAccount {
        match self {
            TokenController::Ft { controller } => controller.account(),
            TokenController::Mt { controller, .. } => controller.account(),
        }
    }
}

impl StorageManagementController for TokenController {}

impl TokenController {
    pub fn ft(contract: TestAccount) -> Self {
        Self::Ft {
            controller: FtController { account: contract },
        }
    }

    pub fn mt(contract: TestAccount, token_id: String) -> Self {
        Self::Mt {
            controller: MtController { account: contract },
            token_id,
        }
    }

    pub async fn mint(&self, account: &TestAccount, amount: impl Into<U128>) -> ExecutionSuccess {
        match self {
            TokenController::Ft { controller } => controller.mint(account, amount).await,
            TokenController::Mt {
                controller,
                token_id,
            } => controller.mint(account, token_id, amount).await,
        }
    }

    pub async fn balance_of(&self, account_id: impl Into<&AccountId>) -> u128 {
        match self {
            TokenController::Ft { controller } => controller.ft_balance_of(account_id).await.0,
            TokenController::Mt {
                controller,
                token_id,
            } => controller.mt_balance_of(token_id, account_id).await.0,
        }
    }

    pub async fn transfer(
        &self,
        sender: &TestAccount,
        receiver_id: impl Into<&AccountId>,
        amount: impl Into<U128>,
    ) -> ExecutionSuccess {
        match self {
            TokenController::Ft { controller } => {
                controller.ft_transfer(sender, receiver_id, amount).await
            }
            TokenController::Mt {
                controller,
                token_id,
            } => {
                controller
                    .mt_transfer(sender, token_id, receiver_id, amount)
                    .await
            }
        }
    }

    pub async fn transfer_call(
        &self,
        sender: &TestAccount,
        receiver_id: impl Into<&AccountId>,
        amount: impl Into<U128>,
        msg: impl Into<String>,
    ) -> ExecutionSuccess {
        match self {
            TokenController::Ft { controller } => {
                controller
                    .ft_transfer_call(sender, receiver_id, amount, msg)
                    .await
            }
            TokenController::Mt {
                controller,
                token_id,
            } => {
                controller
                    .mt_transfer_call(sender, token_id, receiver_id, amount, msg)
                    .await
            }
        }
    }

    pub async fn redemption_rate(&self) -> u128 {
        match self {
            Self::Ft { controller } => controller.redemption_rate().await.0,
            Self::Mt {
                controller,
                token_id,
            } => controller.redemption_rate(token_id).await.0,
        }
    }

    pub async fn set_redemption_rate(&self, redemption_rate: impl Into<U128>) -> ExecutionSuccess {
        match self {
            Self::Ft { controller } => {
                controller
                    .set_redemption_rate(controller.account(), redemption_rate)
                    .await
            }
            Self::Mt {
                controller,
                token_id,
            } => {
                controller
                    .set_redemption_rate(controller.account(), token_id, redemption_rate)
                    .await
            }
        }
    }
}
