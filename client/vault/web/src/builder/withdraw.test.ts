import { describe, it, expect } from "vitest"
import { prepareWithdraw } from "./withdraw.js"
import type { VaultWebContext, WithdrawParams } from "../types/context.js"

describe("prepareWithdraw", () => {
  const ctx: VaultWebContext = {
    networkId: "testnet",
    vaultContractId: "vault.near",
    rpcUrl: "https://rpc.testnet.near.org",
  }

  const params: WithdrawParams = {
    signerId: "user.near",
    receiverId: "user.near",
    amount: "1000000",
  }

  it("includes refresh phase by default (force mode)", async () => {
    const flow = await prepareWithdraw(ctx, params)

    expect(flow.version).toBe(1)
    expect(flow.label).toBe("Withdraw")
    expect(flow.phases).toHaveLength(2)

    expect(flow.phases[0].label).toBe("Refresh markets")
    expect(flow.phases[0].txs[0].tag).toBe("refresh_markets")
    expect(flow.phases[0].barrier).toBe("finalized")

    expect(flow.phases[1].label).toBe("Withdraw from vault")
    expect(flow.phases[1].txs[0].tag).toBe("withdraw")
    expect(flow.phases[1].barrier).toBe("finalized")
  })

  it("skips refresh phase when mode is never", async () => {
    const flow = await prepareWithdraw(ctx, {
      ...params,
      refresh: { mode: "never" },
    })

    expect(flow.phases).toHaveLength(1)
    expect(flow.phases[0].label).toBe("Withdraw from vault")
    expect(flow.phases[0].txs[0].tag).toBe("withdraw")
  })

  it("builds withdraw action with correct args and attached deposit", async () => {
    const flow = await prepareWithdraw(ctx, {
      ...params,
      refresh: { mode: "never" },
    })

    const withdrawTx = flow.phases[0].txs[0]
    expect(withdrawTx.receiverId).toBe("vault.near")

    const action = withdrawTx.actions[0]
    if (action.type === "FunctionCall") {
      expect(action.params.methodName).toBe("withdraw")
      expect(action.params.args).toEqual({
        amount: "1000000",
        receiver: "user.near",
      })
      expect(BigInt(action.params.deposit)).toBeGreaterThan(0n)
    }
  })

  it("passes markets list to refresh_markets", async () => {
    const flow = await prepareWithdraw(ctx, {
      ...params,
      refresh: { mode: "force", markets: [1, 2, 3] },
    })

    const refreshTx = flow.phases[0].txs[0]
    const action = refreshTx.actions[0]
    if (action.type === "FunctionCall") {
      expect(action.params.args).toEqual({ markets: [1, 2, 3] })
    }
  })

  it("throws on zero amount", async () => {
    await expect(prepareWithdraw(ctx, { ...params, amount: "0" })).rejects.toThrow(
      "Withdraw amount must be greater than zero"
    )
  })
})
