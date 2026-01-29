use templar_soroban_vault::{effects::MockInterpreter, EffectContext, EffectInterpreter};
use templar_vault_kernel::{
    effects::KernelEffect,
    state::op_state::{OpState, RefreshingState},
    transitions::{
        allocation_step_callback, complete_allocation, complete_refresh, payout_complete,
        refresh_step_callback, start_allocation, start_refresh, start_withdrawal,
        withdrawal_collected, withdrawal_step_callback, WithdrawalRequest,
    },
};

fn dummy_ctx() -> EffectContext {
    EffectContext::new(0, [1u8; 32], [2u8; 32], [3u8; 32])
}

#[test]
fn deposit_effects_execute() {
    let mut interpreter = MockInterpreter::new();
    let ctx = dummy_ctx();
    let effects = vec![
        KernelEffect::MintShares {
            owner: [9u8; 32],
            shares: 100,
        },
        KernelEffect::EmitEvent {
            event: templar_vault_kernel::effects::KernelEvent::DepositProcessed {
                owner: [8u8; 32],
                receiver: [9u8; 32],
                assets_in: 1000,
                shares_out: 100,
            },
        },
    ];

    let summary = interpreter.execute_effects(&effects, &ctx).unwrap();
    assert_eq!(summary.shares_minted, 100);
    assert_eq!(summary.events_emitted, 1);
    assert_eq!(interpreter.effects.len(), 2);
}

#[test]
fn allocation_flow_reaches_idle() {
    let mut interpreter = MockInterpreter::new();
    let ctx = dummy_ctx();
    let op_id = 7u64;
    let plan = vec![(0u32, 100u128), (1u32, 200u128)];

    let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
    interpreter.execute_effects(&result.effects, &ctx).unwrap();
    let mut state = result.new_state;

    state = allocation_step_callback(state, true, 100, op_id)
        .unwrap()
        .new_state;
    state = allocation_step_callback(state, true, 200, op_id)
        .unwrap()
        .new_state;

    let result = complete_allocation(state, op_id, None).unwrap();
    interpreter.execute_effects(&result.effects, &ctx).unwrap();
    assert!(matches!(result.new_state, OpState::Idle));
}

#[test]
fn refresh_flow_reaches_idle() {
    let mut interpreter = MockInterpreter::new();
    let ctx = dummy_ctx();
    let op_id = 12u64;
    let plan = vec![0u32, 1u32, 2u32];

    let result = start_refresh(OpState::Idle, plan.clone(), op_id).unwrap();
    interpreter.execute_effects(&result.effects, &ctx).unwrap();
    let mut state = result.new_state;

    // simulate each refresh step
    for _ in plan {
        state = refresh_step_callback(state, op_id).unwrap().new_state;
    }

    let result = complete_refresh(state, op_id).unwrap();
    interpreter.execute_effects(&result.effects, &ctx).unwrap();
    assert!(matches!(result.new_state, OpState::Idle));
}

#[test]
fn withdrawal_flow_reaches_idle() {
    let mut interpreter = MockInterpreter::new();
    let ctx = dummy_ctx();
    let op_id = 33u64;

    let request = WithdrawalRequest {
        op_id,
        amount: 150,
        receiver: [6u8; 32],
        owner: [5u8; 32],
        escrow_shares: 150,
    };

    let result = start_withdrawal(OpState::Idle, request).unwrap();
    interpreter.execute_effects(&result.effects, &ctx).unwrap();
    let state = result.new_state;

    let state = withdrawal_step_callback(state, op_id, 150)
        .unwrap()
        .new_state;
    let result = withdrawal_collected(state, op_id, 150).unwrap();
    interpreter.execute_effects(&result.effects, &ctx).unwrap();

    let result = payout_complete(result.new_state, true, op_id).unwrap();
    interpreter.execute_effects(&result.effects, &ctx).unwrap();
    assert!(matches!(result.new_state, OpState::Idle));
}

#[test]
fn refresh_state_roundtrip() {
    let state = OpState::Refreshing(RefreshingState {
        op_id: 9,
        index: 0,
        plan: vec![0, 1],
    });
    let result = refresh_step_callback(state, 9).unwrap();
    assert!(matches!(result.new_state, OpState::Refreshing(_)));
}
