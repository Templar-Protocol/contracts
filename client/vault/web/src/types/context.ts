import type { GasPolicy, StoragePolicy, FtPolicy, RefreshPolicy } from "../policy/defaults.js"
import type { MarketId } from "../abi/generated/types.js"

export type VaultWebContext = {
  readonly networkId: string
  readonly vaultContractId: string
  readonly rpcUrl: string
  readonly policy?: PolicyOverrides
}

export type PolicyOverrides = {
  readonly gas?: Partial<GasPolicy>
  readonly storage?: Partial<StoragePolicy>
  readonly ft?: Partial<FtPolicy>
  readonly refresh?: Partial<RefreshPolicy>
}

export type DepositParams = {
  readonly signerId: string
  readonly amount: string
  readonly warnOnExcessDeposit?: boolean
}

export type WithdrawParams = {
  readonly signerId: string
  readonly receiverId: string
  readonly amount: string
  readonly refresh?: RefreshConfig
}

export type RefreshConfig = {
  readonly mode?: "force" | "never" | "auto"
  readonly markets?: readonly MarketId[]
}

export type RefreshMarketsParams = {
  readonly signerId: string
  readonly markets?: readonly MarketId[]
}

export type { MarketId }
export type {
  VaultConfiguration,
  StorageBalance,
  U128,
  AccountId,
  FungibleAsset,
} from "../abi/generated/types.js"
