use std::fmt::Write;

use near_sdk::AccountId;

#[derive(Debug, thiserror::Error)]
pub enum PayloadRejectionReason {
    #[error("Failed signature verification")]
    SignatureVerificationFailure,
    #[error("Unknown transaction receiver account ID {account_id}")]
    UnknownTransactionReceiverId { account_id: AccountId },
    #[error("Unsupported action at index {index}")]
    UnsupportedAction { index: usize },
    #[error("Function call rejection:{}", ._0.iter().fold(String::new(), |mut a, e| { write!(&mut a, "\n\t{e}").unwrap(); a }))]
    FunctionCallRejection(Vec<FunctionCallRejectionReason>),
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
        "Invalid asset for message at index {index}: expected: {expected}, received: {received}"
    )]
    InvalidAssetForMsg {
        index: usize,
        expected: String,
        received: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_rejection_reason() {
        let e = PayloadRejectionReason::FunctionCallRejection(vec![
            FunctionCallRejectionReason::ArgumentDeserializationFailure { index: 0 },
            FunctionCallRejectionReason::ArgumentDeserializationFailure { index: 1 },
        ]);

        assert_eq!(e.to_string(), "Function call rejection:\n\tArgument deserialization failure at index 0\n\tArgument deserialization failure at index 1");
    }
}
