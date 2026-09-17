//! The unsigned bytes an MPC signature is requested for, in either of NEAR's
//! two signable shapes, and the signed artifact each becomes.

use anyhow::Context as _;
use borsh::BorshDeserialize as _;
use clap::ValueEnum;
use near_account_id::AccountId;
use near_api::types::{
    transaction::{
        actions::Action,
        delegate_action::{DelegateAction, NonDelegateAction, SignedDelegateAction},
        SignedTransaction, Transaction, TransactionV0,
    },
    CryptoHash, PublicKey, Signature,
};
use serde::{Deserialize, Serialize};
use templar_gateway_core::PlannedTransaction;
use templar_gateway_types::{Base64Bytes, BlockSummary, SignedDelegateActionInput};

/// Genesis `transaction_validity_period` on mainnet and testnet: a transaction
/// is accepted only within this many blocks of its `block_hash`.
pub const TRANSACTION_VALIDITY_PERIOD_BLOCKS: u64 = 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    /// A NEP-366 delegate action, relayed later by any account. Expires at an
    /// explicit block height, so it survives a long vote.
    DelegateAction,
    /// A plain transaction, as `near-cli` signs. Bound to a recent block hash,
    /// so it expires `TRANSACTION_VALIDITY_PERIOD_BLOCKS` after proposal.
    Transaction,
}

/// Borsh bytes of the signable, tagged with what they encode. Nothing else:
/// every fact about the payload is read back out of the bytes the hash commits
/// to, so an envelope cannot claim what its payload is not.
///
/// `transaction` bytes are a `TransactionV0`, which is byte-identical to the
/// `Transaction::V0` wire form NEAR hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignablePayload {
    DelegateAction { bytes: Base64Bytes },
    Transaction { bytes: Base64Bytes },
}

pub struct BuildInputs {
    pub kind: PayloadKind,
    pub planned: PlannedTransaction,
    pub public_key: PublicKey,
    pub nonce: u64,
    pub block: BlockSummary,
    pub valid_for_blocks: u64,
}

/// What bounds the signed artifact's acceptance, as the bytes state it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Validity {
    MaxBlockHeight(u64),
    /// Accepted for `TRANSACTION_VALIDITY_PERIOD_BLOCKS` after this block; its
    /// height has to be looked up on chain.
    BlockHash(CryptoHash),
}

/// When the signed artifact stops being accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Expiry {
    MaxBlockHeight {
        last_valid_block: u64,
    },
    BlockHash {
        block_height: u64,
        last_valid_block: u64,
    },
}

impl Expiry {
    pub const fn max_block_height(last_valid_block: u64) -> Self {
        Self::MaxBlockHeight { last_valid_block }
    }

    pub const fn after_block(block_height: u64) -> Self {
        Self::BlockHash {
            block_height,
            last_valid_block: block_height.saturating_add(TRANSACTION_VALIDITY_PERIOD_BLOCKS),
        }
    }

    pub const fn last_valid_block(self) -> u64 {
        match self {
            Self::MaxBlockHeight { last_valid_block }
            | Self::BlockHash {
                last_valid_block, ..
            } => last_valid_block,
        }
    }

    pub const fn is_expired_at(self, head_height: u64) -> bool {
        head_height > self.last_valid_block()
    }
}

/// The payload's fields, for review.
#[derive(Debug, Clone, Serialize)]
pub struct Decoded {
    pub kind: PayloadKind,
    pub signer_id: AccountId,
    pub public_key: PublicKey,
    pub nonce: u64,
    pub receiver_id: AccountId,
    pub actions: Vec<Action>,
    pub validity: Validity,
}

/// The payload with its MPC signature attached.
pub enum Signed {
    DelegateAction(SignedDelegateActionInput),
    Transaction(SignedTransaction),
}

impl SignablePayload {
    pub fn build(inputs: BuildInputs) -> anyhow::Result<Self> {
        let BuildInputs {
            kind,
            planned,
            public_key,
            nonce,
            block,
            valid_for_blocks,
        } = inputs;
        let signer_id = planned.signer_account_id.0;
        match kind {
            PayloadKind::DelegateAction => {
                let actions = planned
                    .actions
                    .into_iter()
                    .map(|action| {
                        NonDelegateAction::try_from(action).map_err(|()| {
                            anyhow::anyhow!("a delegate action cannot nest another delegate action")
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let delegate_action = DelegateAction {
                    sender_id: signer_id,
                    receiver_id: planned.receiver_id,
                    actions,
                    nonce,
                    max_block_height: block.height.saturating_add(valid_for_blocks),
                    public_key,
                };
                Ok(Self::DelegateAction {
                    bytes: Base64Bytes(borsh::to_vec(&delegate_action)?),
                })
            }
            PayloadKind::Transaction => {
                let transaction = TransactionV0 {
                    signer_id,
                    public_key,
                    nonce,
                    receiver_id: planned.receiver_id,
                    block_hash: block.hash.into(),
                    actions: planned.actions,
                };
                Ok(Self::Transaction {
                    bytes: Base64Bytes(borsh::to_vec(&transaction)?),
                })
            }
        }
    }

    /// The 32 bytes the MPC signs: the NEP-461 hash of a delegate action, or
    /// the transaction hash.
    pub fn hash(&self) -> anyhow::Result<[u8; 32]> {
        match self {
            Self::DelegateAction { bytes } => {
                // near-api-types has no NEP-461 hashing; near-primitives shares the borsh layout.
                let delegate_action =
                    near_primitives::action::delegate::DelegateAction::try_from_slice(&bytes.0)
                        .context("decode the delegate action")?;
                Ok(delegate_action.get_nep461_hash().0)
            }
            Self::Transaction { bytes } => Ok(Transaction::V0(transaction_v0(bytes)?).get_hash().0),
        }
    }

    pub fn decode(&self) -> anyhow::Result<Decoded> {
        match self {
            Self::DelegateAction { bytes } => {
                let delegate_action = delegate_action(bytes)?;
                Ok(Decoded {
                    kind: PayloadKind::DelegateAction,
                    signer_id: delegate_action.sender_id,
                    public_key: delegate_action.public_key,
                    nonce: delegate_action.nonce,
                    receiver_id: delegate_action.receiver_id,
                    actions: delegate_action
                        .actions
                        .into_iter()
                        .map(|action| (*action).clone())
                        .collect(),
                    validity: Validity::MaxBlockHeight(delegate_action.max_block_height),
                })
            }
            Self::Transaction { bytes } => {
                let transaction = transaction_v0(bytes)?;
                Ok(Decoded {
                    kind: PayloadKind::Transaction,
                    signer_id: transaction.signer_id,
                    public_key: transaction.public_key,
                    nonce: transaction.nonce,
                    receiver_id: transaction.receiver_id,
                    actions: transaction.actions,
                    validity: Validity::BlockHash(transaction.block_hash),
                })
            }
        }
    }

    /// Attach `signature` after checking it signs this payload's hash under
    /// `public_key` — the derived key the proposal names, not the one embedded
    /// in the bytes, so a payload built for the wrong key is caught here.
    pub fn sign(&self, signature: Signature, public_key: PublicKey) -> anyhow::Result<Signed> {
        let hash = CryptoHash(self.hash()?);
        anyhow::ensure!(
            signature.verify(hash, public_key),
            "the MPC signature does not verify against the derived key {public_key} over hash {hash}"
        );
        match self {
            Self::DelegateAction { bytes } => {
                let signed = SignedDelegateAction {
                    delegate_action: delegate_action(bytes)?,
                    signature,
                };
                Ok(Signed::DelegateAction(
                    SignedDelegateActionInput::from_borsh_bytes(&borsh::to_vec(&signed)?)?,
                ))
            }
            Self::Transaction { bytes } => Ok(Signed::Transaction(SignedTransaction::new(
                signature,
                Transaction::V0(transaction_v0(bytes)?),
            ))),
        }
    }
}

fn delegate_action(bytes: &Base64Bytes) -> anyhow::Result<DelegateAction> {
    DelegateAction::try_from_slice(&bytes.0).context("decode the delegate action")
}

/// near-api-types' `Transaction` derives its borsh decoder, which expects a
/// version tag the V0 wire form does not carry; decode the struct instead.
fn transaction_v0(bytes: &Base64Bytes) -> anyhow::Result<TransactionV0> {
    TransactionV0::try_from_slice(&bytes.0).context("decode the transaction")
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_api::types::transaction::actions::FunctionCallAction;
    use rstest::rstest;
    use templar_gateway_types::{ManagedAccountId, NearGas, NearToken};

    fn planned() -> PlannedTransaction {
        PlannedTransaction {
            signer_account_id: ManagedAccountId("target.near".parse().expect("valid")),
            receiver_id: "market.near".parse().expect("valid"),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "own_get_owner".to_owned(),
                args: b"{}".to_vec(),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            }))],
            continue_on_failure: false,
        }
    }

    fn block() -> BlockSummary {
        BlockSummary {
            height: 1_000,
            timestamp_ns: 0,
            gas_price: NearToken::from_yoctonear(1),
            hash: templar_gateway_types::CryptoHash(CryptoHash([9; 32])),
        }
    }

    fn build(kind: PayloadKind, public_key: PublicKey) -> SignablePayload {
        SignablePayload::build(BuildInputs {
            kind,
            planned: planned(),
            public_key,
            nonce: 42,
            block: block(),
            valid_for_blocks: 500,
        })
        .expect("builds")
    }

    #[rstest]
    #[case(PayloadKind::DelegateAction, Validity::MaxBlockHeight(1_500))]
    #[case(PayloadKind::Transaction, Validity::BlockHash(CryptoHash([9; 32])))]
    fn decodes_what_it_built(#[case] kind: PayloadKind, #[case] validity: Validity) {
        let secret = near_api::signer::generate_secret_key().expect("key");
        let payload = build(kind, secret.public_key());

        let decoded = payload.decode().expect("decodes");
        assert_eq!(decoded.kind, kind);
        assert_eq!(decoded.signer_id.as_str(), "target.near");
        assert_eq!(decoded.receiver_id.as_str(), "market.near");
        assert_eq!(decoded.nonce, 42);
        assert_eq!(decoded.public_key, secret.public_key());
        assert_eq!(decoded.actions, planned().actions);
        assert_eq!(decoded.validity, validity);
    }

    #[rstest]
    #[case(Expiry::max_block_height(1_500), 1_500)]
    #[case(Expiry::after_block(1_000), 1_000 + TRANSACTION_VALIDITY_PERIOD_BLOCKS)]
    fn expiry_is_inclusive_of_its_last_block(
        #[case] expiry: Expiry,
        #[case] last_valid_block: u64,
    ) {
        assert_eq!(expiry.last_valid_block(), last_valid_block);
        assert!(!expiry.is_expired_at(last_valid_block));
        assert!(expiry.is_expired_at(last_valid_block + 1));
    }

    #[rstest]
    #[case(PayloadKind::DelegateAction)]
    #[case(PayloadKind::Transaction)]
    fn envelope_json_round_trips(#[case] kind: PayloadKind) {
        let secret = near_api::signer::generate_secret_key().expect("key");
        let payload = build(kind, secret.public_key());
        let json = serde_json::to_string(&payload).expect("serializes");
        let back: SignablePayload = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, payload);
        assert_eq!(back.hash().expect("hash"), payload.hash().expect("hash"));
    }

    #[test]
    fn transaction_hash_matches_near_primitives() {
        let secret = near_api::signer::generate_secret_key().expect("key");
        let SignablePayload::Transaction { bytes } =
            build(PayloadKind::Transaction, secret.public_key())
        else {
            panic!("built a transaction")
        };
        let reference = near_primitives::transaction::Transaction::V0(
            near_primitives::transaction::TransactionV0::try_from_slice(&bytes.0)
                .expect("same layout"),
        );
        assert_eq!(
            SignablePayload::Transaction { bytes }.hash().expect("hash"),
            reference.get_hash_and_size().0 .0
        );
    }

    #[rstest]
    #[case(PayloadKind::DelegateAction)]
    #[case(PayloadKind::Transaction)]
    fn a_local_signature_over_the_hash_attaches(#[case] kind: PayloadKind) {
        let secret = near_api::signer::generate_secret_key().expect("key");
        let payload = build(kind, secret.public_key());
        let signature = secret.sign(CryptoHash(payload.hash().expect("hash")));

        match payload
            .sign(signature, secret.public_key())
            .expect("verifies")
        {
            Signed::DelegateAction(input) => {
                assert_eq!(input.into_inner().delegate_action.nonce, 42);
            }
            Signed::Transaction(signed) => assert_eq!(signed.transaction.nonce(), 42),
        }
    }

    #[test]
    fn a_signature_under_another_key_is_refused() {
        let secret = near_api::signer::generate_secret_key().expect("key");
        let other = near_api::signer::generate_secret_key().expect("key");
        let payload = build(PayloadKind::DelegateAction, secret.public_key());
        let signature = other.sign(CryptoHash(payload.hash().expect("hash")));
        assert!(payload.sign(signature, secret.public_key()).is_err());
    }

    #[test]
    fn a_delegate_action_cannot_nest_a_delegate_action() {
        let secret = near_api::signer::generate_secret_key().expect("key");
        let inner = build(PayloadKind::DelegateAction, secret.public_key());
        let Signed::DelegateAction(signed) = inner
            .sign(
                secret.sign(CryptoHash(inner.hash().expect("hash"))),
                secret.public_key(),
            )
            .expect("signs")
        else {
            panic!("built a delegate action")
        };
        let mut planned = planned();
        planned.actions = vec![Action::Delegate(Box::new(signed.into_inner()))];

        let error = SignablePayload::build(BuildInputs {
            kind: PayloadKind::DelegateAction,
            planned,
            public_key: secret.public_key(),
            nonce: 1,
            block: block(),
            valid_for_blocks: 1,
        })
        .expect_err("nested delegate");
        assert!(error.to_string().contains("nest"), "{error}");
    }
}
