import { describe, it, expect } from "vitest"
import { prepareRefreshMarkets } from "./refresh.js"
import type { VaultWebContext, RefreshMarketsParams } from "../types/context.js"

describe("prepareRefreshMarkets", () => {
  const ctx: VaultWebContext = {
    networkId: "testnet",
    vaultContractId: "vault.near",
    rpcUrl: "https://rpc.testnet.near.org",
  }

  it("creates single-phase flow with refresh_markets call", async () => {
    const params: RefreshMarketsParams = {
      signerId: "user.near",
    }

    const flow = await prepareRefreshMarkets(ctx, params)

    expect(flow.version).toBe(1)
    expect(flow.label).toBe("Refresh Markets")
    expect(flow.phases).toHaveLength(1)
    expect(flow.phases[0].label).toBe("Refresh markets")
    expect(flow.phases[0].barrier).toBe("finalized")
    expect(flow.phases[0].txs[0].tag).toBe("refresh_markets")
  })

  it("passes empty array when no markets specified (refresh all)", async () => {
    const params: RefreshMarketsParams = {
      signerId: "user.near",
    }

    const flow = await prepareRefreshMarkets(ctx, params)

    const action = flow.phases[0].txs[0].actions[0]
    if (action.type === "FunctionCall") {
      expect(action.params.args).toEqual({ markets: [] })
    }
  })

  it("passes specific markets when provided", async () => {
    const params: RefreshMarketsParams = {
      signerId: "user.near",
      markets: [1, 2],
    }

    const flow = await prepareRefreshMarkets(ctx, params)

    const action = flow.phases[0].txs[0].actions[0]
    if (action.type === "FunctionCall") {
      expect(action.params.args).toEqual({ markets: [1, 2] })
    }
  })

  it("attaches zero deposit (permissionless call)", async () => {
    const params: RefreshMarketsParams = {
      signerId: "user.near",
    }

    const flow = await prepareRefreshMarkets(ctx, params)

    const action = flow.phases[0].txs[0].actions[0]
    if (action.type === "FunctionCall") {
      expect(action.params.deposit).toBe("0")
    }
  })
})
