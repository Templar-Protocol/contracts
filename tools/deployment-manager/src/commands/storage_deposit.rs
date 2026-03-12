use anyhow::Context;
use near_contract_standards::storage_management::StorageBalanceBounds;
use near_fetch::ops::Function;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};

/// `0.00125 NEAR` expressed in yoctoNEAR, a common storage deposit amount.
pub const STORAGE_DEPOSIT_AMOUNT: NearToken =
    NearToken::from_yoctonear(1_250_000_000_000_000_000_000);

/// Deposit storage on `contract_id` for `account_id`.
///
/// Mirrors `script/ci/storage-deposit.sh`.
#[derive(clap::Args, Debug)]
pub struct StorageDeposit {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[arg(long)]
    contract_id: AccountId,
    /// Deposit a specific amount of NEAR tokens.
    #[arg(long)]
    deposit: Option<NearToken>,
    /// Deposit only the minimum storage deposit required by the contract.
    #[arg(long)]
    registration_only: bool,
}

impl StorageDeposit {
    #[tracing::instrument(skip_all, name = "storage_deposit", fields(account_id = %self.signer.account_id, contract_id = %self.contract_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let deposit = if self.registration_only {
            tracing::debug!("Fetching storage balance bounds");
            let bounds = ctx
                .near
                .view(&self.contract_id, "storage_balance_bounds")
                .args_json(b"{}".to_vec())
                .await?
                .json::<StorageBalanceBounds>()?;
            bounds.min
        } else {
            self.deposit.unwrap_or(STORAGE_DEPOSIT_AMOUNT)
        };
        tracing::info!(%deposit, "Depositing storage");

        let signer = self.signer.signer();
        ctx.batch(&signer, &self.contract_id)
            .call(
                Function::new("storage_deposit")
                    .args_json(json!({ "account_id": &self.signer.account_id, "registration_only": self.registration_only }))
                    .deposit(STORAGE_DEPOSIT_AMOUNT),
            )
            .transact()
            .await
            .with_context(|| format!("storage_deposit on {}", self.contract_id))?;

        tracing::info!("Storage deposit complete");
        Ok(())
    }
}
