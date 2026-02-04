use crate::types::Address;

/// Test-only helper for generating tagged addresses.
#[inline]
pub fn addr_with_tag(tag: u8, index: u64) -> Address {
    let mut addr = [0u8; 32];
    addr[0] = tag;
    addr[1..9].copy_from_slice(&index.to_le_bytes());
    addr
}

/// Helper for generating owner addresses in tests.
#[inline]
pub fn owner_addr(index: u64) -> Address {
    addr_with_tag(0x11, index)
}

/// Helper for generating receiver addresses in tests.
#[inline]
pub fn receiver_addr(index: u64) -> Address {
    addr_with_tag(0x22, index)
}
