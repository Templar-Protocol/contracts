/**
 * Prepared flow types for curated vault interactions.
 *
 * A flow consists of ordered phases. Each phase must be finalized before the next begins.
 * This ensures correctness for multi-step operations like storage_deposit -> deposit.
 */

/**
 * A prepared flow ready for execution.
 * Frontend devs receive this from prepare* functions and pass to executeFlow.
 */
export type PreparedFlowV1 = {
  readonly version: 1
  readonly label: string
  readonly phases: readonly PhaseV1[]
  readonly warnings?: readonly string[]
}

/**
 * A phase is a group of transactions that can potentially be batched together.
 * Between phases, the executor must wait for finalization.
 */
export type PhaseV1 = {
  readonly label: string
  /**
   * The barrier type determines how the executor waits between phases.
   * "finalized" means we must wait for the transaction to reach final status.
   */
  readonly barrier: "finalized"
  /**
   * Whether transactions within this phase may be batched into a single wallet call.
   * Even if true, batching is optional and up to the executor/caller.
   */
  readonly batchable: boolean
  readonly txs: readonly PlannedTxV1[]
}

/**
 * A planned transaction ready to be submitted via wallet.
 */
export type PlannedTxV1 = {
  readonly signerId: string
  readonly receiverId: string
  readonly actions: readonly NearActionV1[]
  /**
   * Human-readable tag for UX and debugging.
   * Known tags: "storage_deposit", "refresh_markets", "withdraw", "deposit_ft_transfer_call"
   */
  readonly tag: TxTag
}

export type TxTag =
  | "storage_deposit"
  | "refresh_markets"
  | "withdraw"
  | "deposit_ft_transfer_call"
  | string

/**
 * NEAR action types supported by this package.
 * Matches wallet-selector action format.
 */
export type NearActionV1 =
  | FunctionCallActionV1
  | TransferActionV1

export type FunctionCallActionV1 = {
  readonly type: "FunctionCall"
  readonly params: {
    readonly methodName: string
    readonly args: Record<string, unknown>
    readonly gas: string
    readonly deposit: string
  }
}

export type TransferActionV1 = {
  readonly type: "Transfer"
  readonly params: {
    readonly deposit: string
  }
}

import type { VaultWebError } from "./errors.js"

export type ExecutionResult = {
  readonly success: boolean
  readonly phaseResults: readonly PhaseResult[]
  readonly error?: VaultWebError
}

export type PhaseResult = {
  readonly phaseLabel: string
  readonly txResults: readonly TxResult[]
}

export type TxResult = {
  readonly tag: TxTag
  readonly txHash: string
  readonly success: boolean
  readonly failureReason?: string
}
