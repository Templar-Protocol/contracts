import type {
  PreparedFlowV1,
  PhaseV1,
  ExecutionResult,
  PhaseResult,
  TxResult,
} from "../types/flow.js"
import type { VaultWebContext } from "../types/context.js"
import {
  parseContractError,
  walletRejectedError,
} from "../types/errors.js"
import { NearRpcClient } from "../rpc/client.js"
import { plannedTxsToWalletTxs, type WalletAdapter } from "../wallet/adapter.js"

export type ExecuteFlowOptions = {
  finalityTimeoutMs?: number
  finalityPollIntervalMs?: number
}

export async function executeFlow(
  ctx: VaultWebContext,
  flow: PreparedFlowV1,
  wallet: WalletAdapter,
  options?: ExecuteFlowOptions
): Promise<ExecutionResult> {
  const rpc = new NearRpcClient({
    rpcUrl: ctx.rpcUrl,
    finalityTimeoutMs: options?.finalityTimeoutMs,
    finalityPollIntervalMs: options?.finalityPollIntervalMs,
  })

  const phaseResults: PhaseResult[] = []

  for (const phase of flow.phases) {
    const phaseResult = await executePhase(ctx, phase, wallet, rpc)
    phaseResults.push(phaseResult)

    const failedTx = phaseResult.txResults.find((tx) => !tx.success)
    if (failedTx) {
      const error = parseContractError(failedTx.failureReason ?? "Unknown error", {
        phaseLabel: phase.label,
        txTag: failedTx.tag,
        txHash: failedTx.txHash,
      })

      return {
        success: false,
        phaseResults,
        error,
      }
    }
  }

  return {
    success: true,
    phaseResults,
  }
}

async function executePhase(
  _ctx: VaultWebContext,
  phase: PhaseV1,
  wallet: WalletAdapter,
  rpc: NearRpcClient
): Promise<PhaseResult> {
  const walletTxs = plannedTxsToWalletTxs(phase.txs)

  let txHashes: string[]
  try {
    const result = await wallet.signAndSendTransactions({ transactions: walletTxs })
    txHashes = result.transactionHashes
  } catch (e) {
    const errorMessage = e instanceof Error ? e.message : String(e)
    const isRejection =
      errorMessage.toLowerCase().includes("reject") ||
      errorMessage.toLowerCase().includes("cancel") ||
      errorMessage.toLowerCase().includes("denied")

    const txResults: TxResult[] = phase.txs.map((tx) => ({
      tag: tx.tag,
      txHash: "",
      success: false,
      failureReason: isRejection
        ? walletRejectedError(errorMessage).message
        : errorMessage,
    }))

    return {
      phaseLabel: phase.label,
      txResults,
    }
  }

  if (txHashes.length !== phase.txs.length) {
    const txResults: TxResult[] = phase.txs.map((tx, i) => ({
      tag: tx.tag,
      txHash: txHashes[i] ?? "",
      success: false,
      failureReason: "Transaction hash count mismatch",
    }))

    return {
      phaseLabel: phase.label,
      txResults,
    }
  }

  const txResults: TxResult[] = []

  for (let i = 0; i < phase.txs.length; i++) {
    const tx = phase.txs[i]
    const txHash = txHashes[i]

    const status = await rpc.waitForFinality(txHash, tx.signerId, phase.label)

    txResults.push({
      tag: tx.tag,
      txHash,
      success: status.status === "success",
      failureReason: status.failureReason,
    })

    if (status.status !== "success") {
      break
    }
  }

  return {
    phaseLabel: phase.label,
    txResults,
  }
}
