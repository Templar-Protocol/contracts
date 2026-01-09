import { JsonRpcProvider } from "@near-js/providers"
import { rpcError, rpcTimeoutError, type VaultWebError } from "../types/errors.js"
import type {
  VaultConfiguration,
  StorageBalance,
  U128,
} from "../abi/generated/types.js"

export type RpcClientConfig = {
  readonly rpcUrl: string
  readonly requestTimeoutMs?: number
  readonly finalityTimeoutMs?: number
  readonly finalityPollIntervalMs?: number
}

export type ViewResult<T> =
  | { readonly success: true; readonly value: T }
  | { readonly success: false; readonly error: VaultWebError }

export type TxStatusResult = {
  readonly status: "success" | "failure" | "pending" | "not_found"
  readonly txHash: string
  readonly failureReason?: string
  readonly raw?: unknown
}

export class NearRpcClient {
  private readonly provider: JsonRpcProvider
  private readonly config: Required<RpcClientConfig>

  constructor(config: RpcClientConfig) {
    this.config = {
      rpcUrl: config.rpcUrl,
      requestTimeoutMs: config.requestTimeoutMs ?? 30000,
      finalityTimeoutMs: config.finalityTimeoutMs ?? 120000,
      finalityPollIntervalMs: config.finalityPollIntervalMs ?? 1000,
    }

    this.provider = new JsonRpcProvider(
      { url: this.config.rpcUrl },
      { retries: 2, wait: 500, backoff: 1.5 }
    )
  }

  async viewFunction<T>(
    contractId: string,
    methodName: string,
    args: Record<string, unknown> = {}
  ): Promise<ViewResult<T>> {
    try {
      const result = await this.provider.callFunction(
        contractId,
        methodName,
        args,
        { finality: "optimistic" }
      )
      return { success: true, value: result as T }
    } catch (e) {
      return {
        success: false,
        error: rpcError(e instanceof Error ? e.message : String(e), e),
      }
    }
  }

  async getVaultConfiguration(vaultContractId: string): Promise<ViewResult<VaultConfiguration>> {
    return this.viewFunction<VaultConfiguration>(vaultContractId, "get_configuration", {})
  }

  async getStorageBalance(
    contractId: string,
    accountId: string
  ): Promise<ViewResult<StorageBalance | null>> {
    return this.viewFunction<StorageBalance | null>(contractId, "storage_balance_of", {
      account_id: accountId,
    })
  }

  async getMaxDeposit(vaultContractId: string): Promise<ViewResult<U128>> {
    return this.viewFunction<U128>(vaultContractId, "get_max_deposit", {})
  }

  async waitForFinality(
    txHash: string,
    senderId: string,
    phaseLabel?: string
  ): Promise<TxStatusResult> {
    const startTime = Date.now()
    const timeout = this.config.finalityTimeoutMs
    const pollInterval = this.config.finalityPollIntervalMs

    while (Date.now() - startTime < timeout) {
      const status = await this.getTxStatus(txHash, senderId)

      if (status.status === "success" || status.status === "failure") {
        return status
      }

      if (status.status === "not_found") {
        await this.sleep(pollInterval)
        continue
      }

      await this.sleep(pollInterval)
    }

    return {
      status: "failure",
      txHash,
      failureReason: rpcTimeoutError(phaseLabel ?? "unknown", txHash).message,
    }
  }

  async getTxStatus(txHash: string, senderId: string): Promise<TxStatusResult> {
    try {
      const outcome = await this.provider.viewTransactionStatus(txHash, senderId, "FINAL")
      return this.parseTxOutcome(txHash, outcome)
    } catch (e) {
      const errorStr = String(e).toLowerCase()
      if (errorStr.includes("not found") || errorStr.includes("unknown")) {
        return { status: "not_found", txHash }
      }
      return {
        status: "failure",
        txHash,
        failureReason: e instanceof Error ? e.message : String(e),
        raw: e,
      }
    }
  }

  private parseTxOutcome(txHash: string, outcome: unknown): TxStatusResult {
    const o = outcome as Record<string, unknown>
    const status = o.status as Record<string, unknown> | undefined

    if (status && typeof status === "object" && "SuccessValue" in status) {
      return { status: "success", txHash, raw: outcome }
    }

    if (status && typeof status === "object" && "Failure" in status) {
      return {
        status: "failure",
        txHash,
        failureReason: this.extractFailureMessage(status.Failure),
        raw: outcome,
      }
    }

    const receiptsOutcome = o.receipts_outcome as Array<{ outcome: { status: unknown } }> | undefined
    if (receiptsOutcome) {
      const failedReceipt = receiptsOutcome.find((r) => {
        const s = r.outcome?.status
        return s && typeof s === "object" && "Failure" in s
      })
      if (failedReceipt) {
        const failure = (failedReceipt.outcome.status as { Failure: unknown }).Failure
        return {
          status: "failure",
          txHash,
          failureReason: this.extractFailureMessage(failure),
          raw: outcome,
        }
      }
    }

    return { status: "success", txHash, raw: outcome }
  }

  private extractFailureMessage(failure: unknown): string {
    if (!failure || typeof failure !== "object") {
      return String(failure)
    }

    const actionError = (failure as Record<string, unknown>).ActionError
    if (actionError && typeof actionError === "object") {
      const kind = (actionError as Record<string, unknown>).kind
      if (kind && typeof kind === "object") {
        const funcError = (kind as Record<string, unknown>).FunctionCallError
        if (funcError && typeof funcError === "object") {
          const execError = (funcError as Record<string, unknown>).ExecutionError
          if (typeof execError === "string") {
            return execError
          }
        }
      }
    }

    try {
      return JSON.stringify(failure)
    } catch {
      return String(failure)
    }
  }

  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
  }
}
