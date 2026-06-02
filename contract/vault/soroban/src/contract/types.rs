use crate::effects::EffectSummary;
use crate::storage::SorobanStorage;
use alloc::vec::Vec;
use soroban_sdk::{Address as SdkAddress, Symbol};
use templar_curator_primitives::rbac::RbacAuth;
use templar_vault_kernel::{Address, FeesSpec};

#[derive(Clone)]
pub struct Delta {
    pub market: u32,
    pub amount: u128,
}

#[derive(Clone)]
pub enum AllocationDelta {
    Supply(Delta),
    Withdraw(Delta),
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct DepositResult {
    pub shares_minted: u128,
    pub total_shares: u128,
    pub total_assets: u128,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawRequestResult {
    pub request_id: u64,
    pub shares_escrowed: u128,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct AllocationResult {
    pub op_id: u64,
    pub new_external_assets: u128,
    pub summary: EffectSummary,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshResult {
    pub op_id: u64,
    pub markets_refreshed: u32,
    pub new_external_assets: u128,
}

/// Contract configuration set at initialization.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct ContractConfig {
    /// Curator address.
    pub curator: Address,
    /// Vault contract address.
    pub vault_address: Address,
    /// Allocator addresses (can manage allocations).
    pub allocators: Vec<Address>,
    /// Underlying asset contract address.
    pub asset_address: Address,
    /// Share token contract address.
    pub share_address: Address,
    /// Fee configuration.
    pub fees: FeesSpec,
    /// Virtual share offset passed through to kernel conversion math.
    pub virtual_shares: u128,
    /// Virtual asset offset passed through to kernel conversion math.
    pub virtual_assets: u128,
}

impl ContractConfig {
    /// Create a new contract configuration.
    #[inline]
    #[must_use]
    pub fn new(
        curator: Address,
        vault_address: Address,
        allocators: Vec<Address>,
        asset_address: Address,
        share_address: Address,
    ) -> Self {
        Self {
            curator,
            vault_address,
            allocators,
            asset_address,
            share_address,
            fees: FeesSpec::zero(),
            virtual_shares: 0,
            virtual_assets: 0,
        }
    }

    /// Attach a fees configuration.
    #[inline]
    #[must_use]
    pub fn with_fees(mut self, fees: FeesSpec) -> Self {
        self.fees = fees;
        self
    }

    /// Attach virtual conversion offsets.
    #[inline]
    #[must_use]
    pub fn with_virtual_offsets(mut self, virtual_shares: u128, virtual_assets: u128) -> Self {
        self.virtual_shares = virtual_shares;
        self.virtual_assets = virtual_assets;
        self
    }

    /// Check if the given address is the curator.
    #[inline]
    #[must_use]
    pub fn is_curator(&self, addr: &Address) -> bool {
        &self.curator == addr
    }

    /// Check if the given address is an allocator.
    #[inline]
    #[must_use]
    pub fn is_allocator(&self, addr: &Address) -> bool {
        self.allocators.iter().any(|a| a == addr)
    }

    /// Check if the address has privileged access (curator or allocator).
    #[inline]
    #[must_use]
    pub fn is_privileged(&self, addr: &Address) -> bool {
        self.is_curator(addr) || self.is_allocator(addr)
    }
}

/// Internal storage keys for vault config (instance storage).
/// Using Symbol constants instead of a `#[contracttype]` enum
/// to avoid contractspec bloat and enum conversion codegen.
#[allow(non_upper_case_globals)]
pub struct VaultDataKey;

#[allow(non_upper_case_globals)]
impl VaultDataKey {
    pub const Curator: Symbol = soroban_sdk::symbol_short!("curator");
    pub const Governance: Symbol = soroban_sdk::symbol_short!("govrnce");
    pub const AssetToken: Symbol = soroban_sdk::symbol_short!("asset");
    pub const ShareToken: Symbol = soroban_sdk::symbol_short!("share");
    pub const Sentinel: Symbol = soroban_sdk::symbol_short!("sntnl");
    pub const FeesSpec: Symbol = soroban_sdk::symbol_short!("fees");
    pub const Initialized: Symbol = soroban_sdk::symbol_short!("init");
    pub const Allocators: Symbol = soroban_sdk::symbol_short!("allctrs");
    pub const AllowedAdapters: Symbol = soroban_sdk::symbol_short!("adapters");
    pub const AdapterBindings: Symbol = soroban_sdk::symbol_short!("adapmap");
    pub const SkimRecipient: Symbol = soroban_sdk::symbol_short!("skimrcp");
    pub const VirtualShares: Symbol = soroban_sdk::symbol_short!("vshares");
    pub const VirtualAssets: Symbol = soroban_sdk::symbol_short!("vassets");
    pub const VirtualOffsetsLocked: Symbol = soroban_sdk::symbol_short!("vofflock");
    pub const IdleResyncLastNs: Symbol = soroban_sdk::symbol_short!("idlrsync");
}

pub struct VaultBootstrap<'a> {
    pub config: ContractConfig,
    pub storage: SorobanStorage<'a>,
    pub auth: RbacAuth,
    pub asset_token: SdkAddress,
    pub share_token: SdkAddress,
}
