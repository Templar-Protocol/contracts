use super::*;

#[test]
fn asset_id_from_bytes() {
    let bytes = [42u8; 32];
    let id = AssetId::from_bytes(bytes);
    assert_eq!(id.0, bytes);
}

#[test]
fn asset_id_as_bytes() {
    let bytes = [99u8; 32];
    let id = AssetId(bytes);
    assert_eq!(id.as_bytes(), bytes);
}

#[test]
fn asset_id_roundtrip() {
    let bytes = [123u8; 32];
    let id = AssetId::from_bytes(bytes);
    assert_eq!(id.as_bytes(), bytes);
}

#[test]
fn escrow_settlement_burn_all() {
    let s = EscrowSettlement::burn_all(100);
    assert_eq!(s.to_burn, 100);
    assert_eq!(s.refund, 0);
}

#[test]
fn escrow_settlement_refund_all() {
    let s = EscrowSettlement::refund_all(100);
    assert_eq!(s.to_burn, 0);
    assert_eq!(s.refund, 100);
}

#[test]
fn escrow_settlement_partial() {
    let s = EscrowSettlement::partial(60, 40);
    assert_eq!(s.to_burn, 60);
    assert_eq!(s.refund, 40);
}

#[test]
fn kernel_version_from_into() {
    let v: KernelVersion = 42u32.into();
    assert_eq!(v.0, 42);
    let n: u32 = v.into();
    assert_eq!(n, 42);
}

#[test]
fn asset_id_from_into() {
    let bytes = [1u8; 32];
    let id: AssetId = bytes.into();
    assert_eq!(id.0, bytes);
    let out: [u8; 32] = id.into();
    assert_eq!(out, bytes);
}
