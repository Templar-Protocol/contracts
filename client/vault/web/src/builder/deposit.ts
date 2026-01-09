import type { PreparedFlowV1, PhaseV1, PlannedTxV1 } from "../types/flow.js"
import type { VaultWebContext, DepositParams } from "../types/context.js"
import { policyViolationError } from "../types/errors.js"
import { resolvePolicy } from "../policy/defaults.js"
import { NearRpcClient } from "../rpc/client.js"
import {
  buildStorageDepositAction,
  buildFtTransferCallAction,
  getUnderlyingTokenContractId,
  DEPOSIT_MSG_SUPPLY,
} from "../abi/actions.js"

export async function prepareDeposit(
  ctx: VaultWebContext,
  params: DepositParams
): Promise<PreparedFlowV1> {
  const policy = resolvePolicy(ctx.policy)
  const rpc = new NearRpcClient({ rpcUrl: ctx.rpcUrl })
  const warnings: string[] = []
  const phases: PhaseV1[] = []

  const configResult = await rpc.getVaultConfiguration(ctx.vaultContractId)
  if (!configResult.success) {
    throw configResult.error
  }
  const vaultConfig = configResult.value

  const underlyingTokenId = getUnderlyingTokenContractId(vaultConfig.underlying_token)

  const storageResult = await rpc.getStorageBalance(ctx.vaultContractId, params.signerId)
  if (!storageResult.success) {
    throw storageResult.error
  }

  const needsStorageRegistration = storageResult.value === null

  if (needsStorageRegistration) {
    const storageDepositTx: PlannedTxV1 = {
      signerId: params.signerId,
      receiverId: ctx.vaultContractId,
      actions: [buildStorageDepositAction(policy, params.signerId, true)],
      tag: "storage_deposit",
    }

    phases.push({
      label: "Register share token storage",
      barrier: "finalized",
      batchable: false,
      txs: [storageDepositTx],
    })
  }

  if (params.warnOnExcessDeposit) {
    const maxDepositResult = await rpc.getMaxDeposit(ctx.vaultContractId)
    if (maxDepositResult.success) {
      const maxDeposit = BigInt(maxDepositResult.value)
      const requestedAmount = BigInt(params.amount)
      if (requestedAmount > maxDeposit) {
        warnings.push(
          `Requested deposit (${params.amount}) exceeds vault max capacity (${maxDepositResult.value}). ` +
          `The excess will be refunded automatically.`
        )
      }
    }
  }

  if (BigInt(params.amount) <= 0n) {
    throw policyViolationError("Deposit amount must be greater than zero")
  }

  const depositTx: PlannedTxV1 = {
    signerId: params.signerId,
    receiverId: underlyingTokenId,
    actions: [
      buildFtTransferCallAction(
        policy,
        ctx.vaultContractId,
        params.amount,
        DEPOSIT_MSG_SUPPLY
      ),
    ],
    tag: "deposit_ft_transfer_call",
  }

  phases.push({
    label: "Deposit to vault",
    barrier: "finalized",
    batchable: false,
    txs: [depositTx],
  })

  return {
    version: 1,
    label: "Deposit",
    phases,
    warnings: warnings.length > 0 ? warnings : undefined,
  }
}
