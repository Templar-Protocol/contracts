#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "boundary", derive(near_sdk::BorshStorageKey))]
pub enum VaultStorageKey {
    PendingWithdrawals,
}
