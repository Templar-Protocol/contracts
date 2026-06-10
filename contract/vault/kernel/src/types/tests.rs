#[cfg(feature = "postcard")]
use crate::types::Address;
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

#[cfg(feature = "postcard")]
#[test]
fn postcard_roundtrip_address() {
    let address = Address([9u8; 32]);
    let bytes = postcard::to_allocvec(&address).expect("serialize address");
    let decoded: Address = postcard::from_bytes(&bytes).expect("deserialize address");
    assert_eq!(decoded, address);
}

#[cfg(all(feature = "postcard", feature = "soroban"))]
#[test]
fn soroban_postcard_address_uses_fixed_array_shape() {
    let address = Address([7u8; 32]);
    let bytes = postcard::to_allocvec(&address).expect("serialize address");
    assert_eq!(
        bytes.len(),
        32,
        "expected raw fixed-array postcard encoding"
    );
}

#[cfg(all(feature = "postcard", not(feature = "soroban")))]
#[test]
fn non_soroban_postcard_address_keeps_byte_payload_shape() {
    let address = Address([7u8; 32]);
    let bytes = postcard::to_allocvec(&address).expect("serialize address");
    assert_eq!(bytes.len(), 33, "expected 1-byte length + 32-byte payload");
}
