use near_crypto::Signer;
use near_primitives::transaction::Action;
use near_sdk::AccountId;
use templar_tools_common::near::{self, Client, Function};

/// A NEAR batch transaction bound to a specific context.
///
/// Obtain one via [`crate::CliContext::batch`]. Chain actions with [`call`](Self::call),
/// [`deploy`](Self::deploy), or [`delete_account`](Self::delete_account), then call
/// [`transact`](Self::transact) to execute, log the hash, and propagate failures.
pub struct BoundBatch<'a> {
    transaction_url_prefix: String,
    near: &'a Client,
    signer: &'a Signer,
    receiver_id: AccountId,
    actions: Vec<Action>,
}

impl<'a> BoundBatch<'a> {
    pub(crate) fn new(
        transaction_url_prefix: String,
        near: &'a Client,
        signer: &'a Signer,
        receiver_id: &AccountId,
    ) -> Self {
        Self {
            transaction_url_prefix,
            near,
            signer,
            receiver_id: receiver_id.clone(),
            actions: Vec::new(),
        }
    }

    #[must_use]
    pub fn call(mut self, function: Function) -> Self {
        self.actions.push(function.into());
        self
    }

    #[must_use]
    pub fn deploy(mut self, code: &[u8]) -> Self {
        self.actions.push(near::deploy_action(code));
        self
    }

    #[must_use]
    pub fn delete_account(mut self, beneficiary_id: &AccountId) -> Self {
        self.actions
            .push(near_primitives::transaction::Action::DeleteAccount(
                near_primitives::transaction::DeleteAccountAction {
                    beneficiary_id: beneficiary_id.clone(),
                },
            ));
        self
    }

    /// Execute the transaction, log its hash and explorer URL, and return an error
    /// if execution failed.
    pub async fn transact(self) -> anyhow::Result<()> {
        if self.actions.is_empty() {
            anyhow::bail!("empty batch");
        }

        let result =
            near::send_tx_checked(self.near, self.signer, &self.receiver_id, self.actions).await?;
        let hash = &result.transaction.hash;
        tracing::info!(
            transaction_hash = %hash,
            url = %format!("{}{}", self.transaction_url_prefix, hash),
            "Transaction submitted"
        );
        Ok(())
    }
}
