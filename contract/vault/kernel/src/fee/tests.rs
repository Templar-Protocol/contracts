use super::*;

#[test]
fn test_fee_slot_zero() {
    let slot = FeeSlot::zero();
    assert!(slot.is_zero_rate());
    assert_eq!(slot.recipient, Address([0u8; 32]));
}

#[test]
fn test_fee_slot_new() {
    let recipient = Address([1u8; 32]);
    let slot = FeeSlot::new(Wad::one(), recipient);
    assert!(slot.has_rate());
    assert!(!slot.is_zero_rate());
    assert_eq!(slot.recipient, recipient);
}

#[test]
fn test_fee_slot_default() {
    let slot = FeeSlot::default();
    assert!(!slot.has_rate());
    assert!(slot.is_zero_rate());
    assert_eq!(slot.recipient, Address([0u8; 32]));
}

#[test]
fn test_fees_spec_zero() {
    let fees = FeesSpec::zero();
    assert!(!fees.has_active_slot_fees());
    assert!(!fees.has_growth_cap());
    assert!(fees.is_zero());
}

#[test]
fn test_fees_spec_new() {
    let perf = FeeSlot::new(Wad::one() / 10, Address([1u8; 32])); // 10%
    let mgmt = FeeSlot::new(Wad::one() / 20, Address([2u8; 32])); // 5%
    let fees = FeesSpec::new(perf, mgmt, Some(Wad::one()));
    assert!(fees.has_active_slot_fees());
    assert!(fees.has_growth_cap());
    assert!(!fees.performance.is_zero_rate());
    assert!(!fees.management.is_zero_rate());
    assert!(fees.max_total_assets_growth_rate.is_some());
    assert!(!fees.is_zero());
}

#[test]
fn test_fees_spec_default() {
    let fees = FeesSpec::default();
    assert!(fees.is_zero());
}

#[test]
fn test_generic_fee_with_wad() {
    let fee: Fee<Wad> = Fee::new(Wad::one(), "test.near");
    assert_eq!(fee.fee, Wad::one());
    assert_eq!(fee.recipient, "test.near");
}

#[test]
fn test_generic_fees_with_wad() {
    let fees: Fees<Wad> = Fees::new(
        Fee::new(Wad::one() / 10, "perf.near"),
        Fee::new(Wad::one() / 20, "mgmt.near"),
        None,
    );
    assert!(!fees.performance.fee.is_zero());
    assert!(!fees.management.fee.is_zero());
}

#[cfg(feature = "postcard")]
#[test]
fn postcard_roundtrip_fee_slot() {
    let slot = FeeSlot::new(Wad::one() / 10, Address([7u8; 32]));
    let bytes = postcard::to_allocvec(&slot).expect("serialize fee slot");
    let decoded: FeeSlot = postcard::from_bytes(&bytes).expect("deserialize fee slot");
    assert_eq!(decoded, slot);
}

#[cfg(feature = "postcard")]
#[test]
fn postcard_roundtrip_fees_spec() {
    let fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, Address([1u8; 32])),
        FeeSlot::new(Wad::one() / 20, Address([2u8; 32])),
        Some(Wad::one() / 5),
    );
    let bytes = postcard::to_allocvec(&fees).expect("serialize fees spec");
    let decoded: FeesSpec = postcard::from_bytes(&bytes).expect("deserialize fees spec");
    assert_eq!(decoded, fees);
}

#[cfg(all(feature = "postcard", feature = "soroban"))]
#[test]
fn soroban_postcard_fee_slot_is_compact() {
    let slot = FeeSlot::new(Wad::one() / 10, Address([3u8; 32]));
    let bytes = postcard::to_allocvec(&slot).expect("serialize fee slot");
    assert!(
        bytes.len() < 50,
        "expected compact fee slot encoding, got {} bytes",
        bytes.len()
    );
}
