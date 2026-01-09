/**
 * Type exports for @templar/vault-web
 */

export type {
  PreparedFlowV1,
  PhaseV1,
  PlannedTxV1,
  TxTag,
  NearActionV1,
  FunctionCallActionV1,
  TransferActionV1,
  ExecutionResult,
  PhaseResult,
  TxResult,
} from "./flow.js"

export type {
  VaultWebContext,
  PolicyOverrides,
  DepositParams,
  WithdrawParams,
  RefreshConfig,
  RefreshMarketsParams,
  MarketId,
  VaultConfiguration,
  StorageBalance,
  U128,
  AccountId,
  FungibleAsset,
} from "./context.js"

export {
  VaultWebError,
  parseContractError,
  rpcTimeoutError,
  rpcError,
  walletRejectedError,
  policyViolationError,
} from "./errors.js"

export type {
  VaultWebErrorCode,
  ErrorContext,
} from "./errors.js"
