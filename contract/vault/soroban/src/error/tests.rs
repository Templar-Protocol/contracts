use super::*;

#[test]
fn test_error_constructors() {
    let err = RuntimeError::unauthorized("not allowed");
    assert!(matches!(err, RuntimeError::Unauthorized));

    let err = RuntimeError::insufficient_balance(100, 200);
    assert!(matches!(err, RuntimeError::InsufficientBalance));

    let err = RuntimeError::invalid_state("wrong state");
    assert!(matches!(err, RuntimeError::InvalidState));

    let err = RuntimeError::storage_error("storage failed");
    assert!(matches!(err, RuntimeError::StorageError));

    let err = RuntimeError::effect_failed("effect failed");
    assert!(matches!(err, RuntimeError::EffectFailed));

    let err = RuntimeError::invalid_input("bad input");
    assert!(matches!(err, RuntimeError::InvalidInput));

    let err = RuntimeError::kernel_error("kernel failed");
    assert!(matches!(err, RuntimeError::KernelError));

    let err = RuntimeError::contract_error("contract error");
    assert!(matches!(err, RuntimeError::InvalidState));

    let err = RuntimeError::transition_error();
    assert!(matches!(err, RuntimeError::KernelError));
}
