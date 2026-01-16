/**
 * Default policies for gas, storage deposits, and behavior.
 * These are the curated defaults maintained by contract builders.
 * All values can be overridden via PolicyOverrides.
 */

/**
 * Gas policy - default gas amounts for each contract method.
 * Values are in gas units (string format).
 */
export type GasPolicy = {
  /** Gas for refresh_markets call */
  readonly refresh_markets: string
  /** Gas for withdraw call */
  readonly withdraw: string
  /** Gas for storage_deposit call on share token */
  readonly storage_deposit: string
  /** Gas for ft_transfer_call to underlying token */
  readonly ft_transfer_call: string
}

/**
 * Default gas values.
 * These are conservative estimates based on contract gas constants.
 */
export const DEFAULT_GAS: GasPolicy = {
  // refresh_markets can span multiple markets and callbacks
  refresh_markets: "100000000000000", // 100 TGas
  // withdraw enqueues a pending withdrawal
  withdraw: "50000000000000", // 50 TGas
  // storage_deposit is relatively cheap
  storage_deposit: "10000000000000", // 10 TGas
  // ft_transfer_call needs gas for the transfer + callback
  ft_transfer_call: "50000000000000", // 50 TGas
}

/**
 * Storage policy - yoctoNEAR amounts for storage deposits.
 */
export type StoragePolicy = {
  /**
   * Attached deposit for withdraw() to cover pending withdrawal queue entry.
   * Conservative estimate: ~500 bytes * 10^19 yoctoNEAR/byte = 5 * 10^21 yoctoNEAR
   * Contract uses storage_bytes_for_pending_withdrawal() internally.
   */
  readonly withdraw_request_yocto: string
  /**
   * Deposit for share token storage_deposit (NEP-145 registration).
   * Contract sets StorageBalanceBounds { min: 2 milliNEAR }.
   */
  readonly share_storage_deposit_yocto: string
}

/**
 * Default storage deposit values.
 * 
 * Contract storage calculations (at 10^19 yoctoNEAR/byte):
 * - MAP_ENTRY_OVERHEAD = 64 bytes
 * - VEC_ITEM_OVERHEAD = 16 bytes  
 * - storage_bytes_for_account_id() = 4 + 64 = 68 bytes (length prefix + max AccountId)
 * - U128_BYTES = 16 bytes
 * - U64_BYTES = 8 bytes
 */
export const DEFAULT_STORAGE: StoragePolicy = {
  // Pending withdrawal entry storage:
  // key(8) + PendingWithdrawal(owner:68 + receiver:68 + escrow_shares:16 + expected_assets:16 + requested_at:8 = 176)
  // Total: MAP_ENTRY_OVERHEAD(64) + 8 + 176 = 248 bytes
  // 248 * 10^19 = 2.48 * 10^21 yoctoNEAR
  // Using 3 milliNEAR for safety margin
  withdraw_request_yocto: "3000000000000000000000", // 3 milliNEAR

  // Share token storage (NEP-145 registration):
  // MAP_ENTRY_OVERHEAD(64) + account_id(68) + balance(16) = 148 bytes
  // 148 * 10^19 = 1.48 * 10^21 yoctoNEAR
  // Using 1.5 milliNEAR (matches contract's yocto_for_ft_account())
  share_storage_deposit_yocto: "1500000000000000000000", // 1.5 milliNEAR
}

/**
 * FT (fungible token) policy - for underlying token interactions.
 */
export type FtPolicy = {
  /**
   * Attached deposit for ft_transfer_call.
   * Standard NEP-141 requires 1 yoctoNEAR.
   */
  readonly ft_transfer_call_attached_yocto: string
}

/**
 * Default FT policy values.
 */
export const DEFAULT_FT: FtPolicy = {
  ft_transfer_call_attached_yocto: "1", // 1 yoctoNEAR (standard)
}

/**
 * Refresh policy - controls refresh_markets behavior in flows.
 */
export type RefreshPolicy = {
  /**
   * Default refresh mode for withdraw flows.
   * - "force": always include refresh phase before withdraw
   * - "never": never include refresh phase
   * - "auto": use policy heuristics (currently defaults to "force" for safety)
   */
  readonly withdraw_refresh_mode: "force" | "never" | "auto"
}

/**
 * Default refresh policy.
 */
export const DEFAULT_REFRESH: RefreshPolicy = {
  // Default to "force" for safety since refresh is often required
  // before withdrawals to ensure market data is fresh
  withdraw_refresh_mode: "force",
}

/**
 * Merged policy with all defaults.
 */
export type ResolvedPolicy = {
  readonly gas: GasPolicy
  readonly storage: StoragePolicy
  readonly ft: FtPolicy
  readonly refresh: RefreshPolicy
}

/**
 * Get the full resolved policy by merging overrides with defaults.
 */
export function resolvePolicy(overrides?: {
  gas?: Partial<GasPolicy>
  storage?: Partial<StoragePolicy>
  ft?: Partial<FtPolicy>
  refresh?: Partial<RefreshPolicy>
}): ResolvedPolicy {
  return {
    gas: {
      ...DEFAULT_GAS,
      ...overrides?.gas,
    },
    storage: {
      ...DEFAULT_STORAGE,
      ...overrides?.storage,
    },
    ft: {
      ...DEFAULT_FT,
      ...overrides?.ft,
    },
    refresh: {
      ...DEFAULT_REFRESH,
      ...overrides?.refresh,
    },
  }
}
