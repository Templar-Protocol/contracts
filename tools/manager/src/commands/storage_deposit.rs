use anyhow::Context;
use near_contract_standards::storage_management::StorageBalanceBounds;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};
use templar_tools_common::near::{self, Function};

/// `0.00125 NEAR` expressed in yoctoNEAR, a common storage deposit amount.
pub const STORAGE_DEPOSIT_AMOUNT: NearToken =
    NearToken::from_yoctonear(1_250_000_000_000_000_000_000);

/// Deposit storage on --contract-id for --signer-id.
#[derive(clap::Args, Debug)]
pub struct StorageDeposit {
    /// Signer for the deposit transaction. Storage is deposited on behalf of this same account.
    #[command(flatten)]
    pub signer: super::SignerArgs,
    /// The contract to deposit storage on.
    #[arg(long)]
    pub contract_id: AccountId,
    /// Deposit a specific amount of NEAR tokens.
    ///
    /// If neither --deposit nor --registration-only are provided, the default
    /// storage amount of 0.00125 NEAR will be used.
    #[arg(long, conflicts_with = "registration_only")]
    pub deposit: Option<NearToken>,
    /// Deposit only the minimum storage deposit required by the contract.
    ///
    /// Conflicts with `--deposit`.
    #[arg(long, conflicts_with = "deposit")]
    pub registration_only: bool,
}

impl StorageDeposit {
    #[tracing::instrument(skip_all, name = "storage_deposit", fields(account_id = %self.signer.account_id, contract_id = %self.contract_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let deposit = if self.registration_only {
            tracing::debug!("Fetching storage balance bounds");
            let bounds: StorageBalanceBounds = near::view(
                &ctx.near,
                &self.contract_id,
                "storage_balance_bounds",
                json!({}),
            )
            .await?;
            bounds.min
        } else {
            self.deposit.unwrap_or(STORAGE_DEPOSIT_AMOUNT)
        };
        tracing::info!(%deposit, "Depositing storage");

        let signer = self.signer.signer();
        ctx.batch(&signer, &self.contract_id)
            .call(Function::new("storage_deposit").args_json(
                json!({ "account_id": &self.signer.account_id, "registration_only": self.registration_only }),
            )?
            .deposit(deposit))
            .transact()
            .await
            .with_context(|| format!("storage_deposit on {}", self.contract_id))?;

        tracing::info!("Storage deposit complete");
        Ok(())
    }
}
