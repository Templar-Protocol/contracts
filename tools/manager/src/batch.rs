use std::marker::PhantomData;

use near_crypto::Signer;
use near_primitives::transaction::{Action, DeleteAccountAction};
use near_sdk::AccountId;
use templar_tools_common::near::{self, Client, Function};

/// Typestate for a batch that can still accept actions.
pub struct Open;

/// Typestate for a batch whose terminal action has already been added.
pub struct Closed;

/// A NEAR batch transaction bound to a specific context.
///
/// Obtain one via [`crate::CliContext::batch`].
pub struct BoundBatch<'a, State = Open> {
    transaction_url_prefix: String,
    near: &'a Client,
    signer: &'a Signer,
    receiver_id: AccountId,
    actions: Vec<Action>,
    state: PhantomData<State>,
}

impl<'a> BoundBatch<'a, Open> {
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
            state: PhantomData,
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
    pub fn delete_account(mut self, beneficiary_id: &AccountId) -> BoundBatch<'a, Closed> {
        self.actions
            .push(Action::DeleteAccount(DeleteAccountAction {
                beneficiary_id: beneficiary_id.clone(),
            }));
        self.close()
    }

    fn close(self) -> BoundBatch<'a, Closed> {
        BoundBatch {
            transaction_url_prefix: self.transaction_url_prefix,
            near: self.near,
            signer: self.signer,
            receiver_id: self.receiver_id,
            actions: self.actions,
            state: PhantomData,
        }
    }
}

impl<State> BoundBatch<'_, State> {
    /// Execute the transaction, log its hash and explorer URL, and return an error
    /// if execution failed.
    pub async fn transact(self) -> anyhow::Result<()> {
        if self.actions.is_empty() {
            anyhow::bail!("empty batch");
        }

        let result = near::send_tx(self.near, self.signer, &self.receiver_id, self.actions).await?;
        let hash = &result.transaction.hash;
        tracing::info!(
            transaction_hash = %hash,
            url = %format!("{}{}", self.transaction_url_prefix, hash),
            "Transaction submitted"
        );
        near::require_success_status(&result)?;
        Ok(())
    }
}
