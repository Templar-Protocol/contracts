use near_sdk::AccountId;

#[derive(Debug, thiserror::Error)]
#[error("Failed precondition: ")]
pub enum PreconditionError {
    #[error("Failed signature verification")]
    SignatureVerificationFailure,
    #[error("Unknown transaction receiver account ID {account_id}")]
    UnknownTransactionReceiverId { account_id: AccountId },
    #[error("Unsupported action at index {index}")]
    UnsupportedAction { index: usize },
    #[error("Argument deserialization failure at index {index}")]
    ArgumentDeserializationFailure { index: usize },
    #[error("Msg deserialization failure at index {index}")]
    MsgDeserializationFailure { index: usize },
    #[error("Unknown token transfer receiver account ID {account_id} at index {index}")]
    UnknownTransferReceiverId { account_id: AccountId, index: usize },
    #[error("Invalid message for asset at index {index}")]
    InvalidMsgForAsset { index: usize },
    #[error("Unknown function name `{name}` at index {index}")]
    UnknownFunctionName { name: String, index: usize },
}
