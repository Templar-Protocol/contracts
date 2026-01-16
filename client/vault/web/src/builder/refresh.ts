import type { PreparedFlowV1, PlannedTxV1 } from "../types/flow.js"
import type { VaultWebContext, RefreshMarketsParams } from "../types/context.js"
import { resolvePolicy } from "../policy/defaults.js"
import { buildRefreshMarketsAction } from "../abi/actions.js"

export async function prepareRefreshMarkets(
  ctx: VaultWebContext,
  params: RefreshMarketsParams
): Promise<PreparedFlowV1> {
  const policy = resolvePolicy(ctx.policy)
  const markets = params.markets ?? []

  const refreshTx: PlannedTxV1 = {
    signerId: params.signerId,
    receiverId: ctx.vaultContractId,
    actions: [buildRefreshMarketsAction(policy, markets)],
    tag: "refresh_markets",
  }

  return {
    version: 1,
    label: "Refresh Markets",
    phases: [
      {
        label: "Refresh markets",
        barrier: "finalized",
        batchable: false,
        txs: [refreshTx],
      },
    ],
  }
}
