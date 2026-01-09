import type { PreparedFlowV1, PhaseV1, PlannedTxV1 } from "../types/flow.js"
import type { VaultWebContext, WithdrawParams, MarketId } from "../types/context.js"
import { policyViolationError } from "../types/errors.js"
import { resolvePolicy } from "../policy/defaults.js"
import { buildWithdrawAction, buildRefreshMarketsAction } from "../abi/actions.js"

export async function prepareWithdraw(
  ctx: VaultWebContext,
  params: WithdrawParams
): Promise<PreparedFlowV1> {
  const policy = resolvePolicy(ctx.policy)
  const warnings: string[] = []
  const phases: PhaseV1[] = []

  if (BigInt(params.amount) <= 0n) {
    throw policyViolationError("Withdraw amount must be greater than zero")
  }

  const refreshMode = params.refresh?.mode ?? policy.refresh.withdraw_refresh_mode
  const shouldRefresh = refreshMode === "force" || refreshMode === "auto"

  if (shouldRefresh) {
    const markets: readonly MarketId[] = params.refresh?.markets ?? []

    const refreshTx: PlannedTxV1 = {
      signerId: params.signerId,
      receiverId: ctx.vaultContractId,
      actions: [buildRefreshMarketsAction(policy, markets)],
      tag: "refresh_markets",
    }

    phases.push({
      label: "Refresh markets",
      barrier: "finalized",
      batchable: false,
      txs: [refreshTx],
    })
  }

  const withdrawTx: PlannedTxV1 = {
    signerId: params.signerId,
    receiverId: ctx.vaultContractId,
    actions: [buildWithdrawAction(policy, params.amount, params.receiverId)],
    tag: "withdraw",
  }

  phases.push({
    label: "Withdraw from vault",
    barrier: "finalized",
    batchable: false,
    txs: [withdrawTx],
  })

  return {
    version: 1,
    label: "Withdraw",
    phases,
    warnings: warnings.length > 0 ? warnings : undefined,
  }
}
