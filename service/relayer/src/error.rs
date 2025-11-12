use near_sdk::AccountId;

#[derive(Debug, thiserror::Error)]
pub enum PayloadRejectionReason {
    #[error("Failed signature verification")]
    SignatureVerificationFailure,
    #[error("Unknown transaction receiver account ID {account_id}")]
    UnknownTransactionReceiverId { account_id: AccountId },
    #[error("Unsupported action at index {index}")]
    UnsupportedAction { index: usize },
    #[error("Function call rejection: {0}")]
    FunctionCallRejection(#[from] FunctionCallRejectionReason),
    #[error("Function call rejection: {}", ._0.iter().map(|e| e.to_string() + "\n").collect::<String>())]
    FunctionCallRejections(Vec<FunctionCallRejectionReason>),
}

#[derive(Debug, thiserror::Error)]
pub enum FunctionCallRejectionReason {
    #[error("Unknown function name \"{function_name}\" at index {index}")]
    UnknownFunctionName { index: usize, function_name: String },
    #[error("Unknown token transfer receiver account ID {account_id} at index {index}")]
    UnknownTransferReceiverId { account_id: AccountId, index: usize },
    #[error("Argument deserialization failure at index {index}")]
    ArgumentDeserializationFailure { index: usize },
    #[error("Msg deserialization failure at index {index}: {msg}")]
    MsgDeserializationFailure { index: usize, msg: String },
    #[error(
        "Invalid message for asset at index {index}: expected: {expected}, actual: \"{actual}\""
    )]
    InvalidMsgForAsset {
        index: usize,
        expected: String,
        actual: String,
    },
}
