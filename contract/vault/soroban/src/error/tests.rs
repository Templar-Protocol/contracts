use super::*;

#[test]
fn test_error_constructors() {
    let err = RuntimeError::unauthorized("not allowed");
    assert!(matches!(err, RuntimeError::Unauthorized(_)));

    let err = RuntimeError::insufficient_balance(100, 200);
    assert!(matches!(
        err,
        RuntimeError::InsufficientBalance {
            available: 100,
            required: 200
        }
    ));

    let err = RuntimeError::invalid_state("wrong state");
    assert!(matches!(err, RuntimeError::InvalidState(_)));

    let err = RuntimeError::storage_error("storage failed");
    assert!(matches!(err, RuntimeError::StorageError(_)));

    let err = RuntimeError::effect_failed("effect failed");
    assert!(matches!(err, RuntimeError::EffectFailed(_)));

    let err = RuntimeError::invalid_input("bad input");
    assert!(matches!(err, RuntimeError::InvalidInput(_)));

    let err = RuntimeError::kernel_error("kernel failed");
    assert!(matches!(err, RuntimeError::KernelError(_)));

    let err = RuntimeError::contract_error("contract error");
    assert!(matches!(err, RuntimeError::InvalidState(_)));

    let err = RuntimeError::transition_error("transition failed");
    assert!(matches!(err, RuntimeError::KernelError(_)));
}
