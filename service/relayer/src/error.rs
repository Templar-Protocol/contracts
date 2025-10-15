use near_sdk::AccountId;

#[derive(Debug, thiserror::Error)]
pub enum PreconditionError {
    #[error("Failed signature verification")]
    SignatureVerificationFailure,
    #[error("Unknown transaction receiver account ID {account_id}")]
    UnknownTransactionReceiverId { account_id: AccountId },
    #[error("Unsupported action at index {index}: {action:?}")]
    UnsupportedAction {
        index: usize,
        action: near_primitives::action::Action,
    },
    #[error("Argument deserialization failure at index {index}")]
    ArgumentDeserializationFailure { index: usize },
    #[error("Msg deserialization failure at index {index}: {msg}")]
    MsgDeserializationFailure { index: usize, msg: String },
    #[error("Unknown token transfer receiver account ID {account_id} at index {index}")]
    UnknownTransferReceiverId { account_id: AccountId, index: usize },
    #[error(
        "Invalid message for asset at index {index}: expected: {expected}, actual: \"{actual}\""
    )]
    InvalidMsgForAsset {
        index: usize,
        expected: String,
        actual: String,
    },
    #[error("Unknown function name at index {index}")]
    UnknownFunctionName { index: usize },
}
