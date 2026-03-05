use crate::types::EscrowSettlement;

#[test]
fn from_escrow_and_burn_clamps_to_escrow() {
    let settlement = EscrowSettlement::from_escrow_and_burn(100, 200);
    assert_eq!(settlement.to_burn, 100);
    assert_eq!(settlement.refund, 0);
}

#[test]
fn from_escrow_and_burn_refunds_remainder() {
    let settlement = EscrowSettlement::from_escrow_and_burn(100, 40);
    assert_eq!(settlement.to_burn, 40);
    assert_eq!(settlement.refund, 60);
}

#[test]
fn from_escrow_and_burn_handles_zero_escrow() {
    let settlement = EscrowSettlement::from_escrow_and_burn(0, 50);
    assert_eq!(settlement.to_burn, 0);
    assert_eq!(settlement.refund, 0);
}
