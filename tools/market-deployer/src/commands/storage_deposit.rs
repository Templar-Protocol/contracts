use anyhow::Context;
use near_contract_standards::storage_management::StorageBalanceBounds;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};

/// `0.00125 NEAR` expressed in yoctoNEAR — the standard NEP-141 storage deposit.
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
    #[arg(long)]
    deposit: Option<NearToken>,
    #[arg(long)]
    minimum: bool,
}

impl StorageDeposit {
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let deposit = if self.minimum {
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

        ctx.near
            .call(&self.signer.signer(), &self.contract_id, "storage_deposit")
            .args_json(json!({ "account_id": &self.signer.account_id }))
            .deposit(STORAGE_DEPOSIT_AMOUNT)
            .transact()
            .await
            .with_context(|| format!("storage_deposit on {}", self.contract_id))?
            .into_result()
            .with_context(|| format!("storage_deposit execution on {}", self.contract_id))?;

        tracing::info!("Storage deposit complete");
        Ok(())
    }
}
