use near_sdk::{json_types::U128, AccountId};
use near_workspaces::{result::ExecutionSuccess, Account, Contract};

use crate::FtController;

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
    fn contract(&self) -> &Contract {
        match self {
            TokenController::Ft { controller } => controller.contract(),
            TokenController::Mt { controller, .. } => controller.contract(),
        }
    }
}

impl TokenController {
    pub fn ft(contract: Contract) -> Self {
        Self::Ft {
            controller: FtController { contract },
        }
    }

    pub fn mt(contract: Contract, token_id: String) -> Self {
        Self::Mt {
            controller: MtController { contract },
            token_id,
        }
    }

    pub async fn mint(&self, account: &Account, amount: impl Into<U128>) -> ExecutionSuccess {
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
        sender: &Account,
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
        sender: &Account,
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

    pub async fn set_redemption_rate(&self, redemption_rate: impl Into<U128>) {
        match self {
            Self::Ft { controller } => {
                controller
                    .set_redemption_rate(controller.contract.as_account(), redemption_rate)
                    .await;
            }
            Self::Mt {
                controller,
                token_id,
            } => {
                controller
                    .set_redemption_rate(
                        controller.contract.as_account(),
                        token_id,
                        redemption_rate,
                    )
                    .await;
            }
        }
    }
}
