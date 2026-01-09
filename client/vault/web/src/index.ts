import type { VaultWebContext, DepositParams, WithdrawParams, RefreshMarketsParams } from "./types/context.js"
import type { ExecutionResult } from "./types/flow.js"
import type { WalletAdapter } from "./wallet/adapter.js"
import { prepareDeposit } from "./builder/deposit.js"
import { prepareWithdraw } from "./builder/withdraw.js"
import { prepareRefreshMarkets } from "./builder/refresh.js"
import { executeFlow, type ExecuteFlowOptions } from "./executor/execute.js"

export async function deposit(
  ctx: VaultWebContext,
  params: DepositParams,
  wallet: WalletAdapter,
  options?: ExecuteFlowOptions
): Promise<ExecutionResult> {
  const flow = await prepareDeposit(ctx, params)
  return executeFlow(ctx, flow, wallet, options)
}

export async function withdraw(
  ctx: VaultWebContext,
  params: WithdrawParams,
  wallet: WalletAdapter,
  options?: ExecuteFlowOptions
): Promise<ExecutionResult> {
  const flow = await prepareWithdraw(ctx, params)
  return executeFlow(ctx, flow, wallet, options)
}

export async function refreshMarkets(
  ctx: VaultWebContext,
  params: RefreshMarketsParams,
  wallet: WalletAdapter,
  options?: ExecuteFlowOptions
): Promise<ExecutionResult> {
  const flow = await prepareRefreshMarkets(ctx, params)
  return executeFlow(ctx, flow, wallet, options)
}

export { prepareDeposit, prepareWithdraw, prepareRefreshMarkets } from "./builder/index.js"

export { executeFlow } from "./executor/index.js"
export type { ExecuteFlowOptions } from "./executor/index.js"

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
  VaultWebErrorCode,
  ErrorContext,
} from "./types/index.js"

export {
  VaultWebError,
  parseContractError,
  rpcTimeoutError,
  rpcError,
  walletRejectedError,
  policyViolationError,
} from "./types/index.js"

export type {
  GasPolicy,
  StoragePolicy,
  FtPolicy,
  RefreshPolicy,
  ResolvedPolicy,
} from "./policy/index.js"

export {
  DEFAULT_GAS,
  DEFAULT_STORAGE,
  DEFAULT_FT,
  DEFAULT_REFRESH,
  resolvePolicy,
} from "./policy/index.js"

export type {
  WalletAdapter,
  WalletSelectorAction,
  WalletSelectorTransaction,
  SignAndSendResult,
} from "./wallet/index.js"

export {
  plannedTxToWalletTx,
  plannedTxsToWalletTxs,
  actionToWalletAction,
} from "./wallet/index.js"

export { NearRpcClient } from "./rpc/index.js"
export type { RpcClientConfig, ViewResult, TxStatusResult } from "./rpc/index.js"

export {
  DEPOSIT_MSG_SUPPLY,
  buildStorageDepositAction,
  buildWithdrawAction,
  buildRefreshMarketsAction,
  buildFtTransferCallAction,
  getUnderlyingTokenContractId,
} from "./abi/index.js"
