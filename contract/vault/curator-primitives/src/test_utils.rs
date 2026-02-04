use templar_vault_kernel::Address;

#[inline]
pub(crate) fn addr_with_tag(tag: u8, index: u64) -> Address {
    let mut addr = [0u8; 32];
    addr[0] = tag;
    addr[1..9].copy_from_slice(&index.to_le_bytes());
    addr
}

#[inline]
pub(crate) fn owner_addr(index: u64) -> Address {
    addr_with_tag(0x11, index)
}

#[inline]
pub(crate) fn receiver_addr(index: u64) -> Address {
    addr_with_tag(0x22, index)
}
