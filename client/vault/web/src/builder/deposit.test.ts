import { describe, it, expect, vi, beforeEach } from "vitest"
import { prepareDeposit } from "./deposit.js"
import type { VaultWebContext, DepositParams, VaultConfiguration, StorageBalance } from "../types/context.js"
import { NearRpcClient } from "../rpc/client.js"
import { DEPOSIT_MSG_SUPPLY } from "../abi/actions.js"

vi.mock("../rpc/client.js", () => ({
  NearRpcClient: vi.fn(),
}))

const mockVaultConfig: VaultConfiguration = {
  owner: "owner.near",
  curator: "curator.near",
  guardian: "guardian.near",
  sentinel: "sentinel.near",
  underlying_token: {
    Nep141: "usdc.near",
  },
  initial_timelock_ns: "86400000000000",
  fees: {
    performance: { fee: "0", recipient: "fee.near" },
    management: { fee: "0", recipient: "fee.near" },
  },
  skim_recipient: "skim.near",
  name: "Test Vault",
  symbol: "TV",
  decimals: 6,
  restrictions: null,
}

describe("prepareDeposit", () => {
  const ctx: VaultWebContext = {
    networkId: "testnet",
    vaultContractId: "vault.near",
    rpcUrl: "https://rpc.testnet.near.org",
  }

  const params: DepositParams = {
    signerId: "user.near",
    amount: "1000000",
  }

  let mockRpc: {
    getVaultConfiguration: ReturnType<typeof vi.fn>
    getStorageBalance: ReturnType<typeof vi.fn>
    getMaxDeposit: ReturnType<typeof vi.fn>
  }

  beforeEach(() => {
    mockRpc = {
      getVaultConfiguration: vi.fn(),
      getStorageBalance: vi.fn(),
      getMaxDeposit: vi.fn(),
    }
    vi.mocked(NearRpcClient).mockImplementation(() => mockRpc as unknown as NearRpcClient)
  })

  it("includes storage_deposit phase when user is not registered", async () => {
    mockRpc.getVaultConfiguration.mockResolvedValue({ success: true, value: mockVaultConfig })
    mockRpc.getStorageBalance.mockResolvedValue({ success: true, value: null })

    const flow = await prepareDeposit(ctx, params)

    expect(flow.version).toBe(1)
    expect(flow.label).toBe("Deposit")
    expect(flow.phases).toHaveLength(2)

    expect(flow.phases[0].label).toBe("Register share token storage")
    expect(flow.phases[0].txs[0].tag).toBe("storage_deposit")
    expect(flow.phases[0].barrier).toBe("finalized")

    expect(flow.phases[1].label).toBe("Deposit to vault")
    expect(flow.phases[1].txs[0].tag).toBe("deposit_ft_transfer_call")
  })

  it("skips storage_deposit phase when user is already registered", async () => {
    const storageBalance: StorageBalance = { total: "2000000000000000000000", available: "0" }
    mockRpc.getVaultConfiguration.mockResolvedValue({ success: true, value: mockVaultConfig })
    mockRpc.getStorageBalance.mockResolvedValue({ success: true, value: storageBalance })

    const flow = await prepareDeposit(ctx, params)

    expect(flow.phases).toHaveLength(1)
    expect(flow.phases[0].label).toBe("Deposit to vault")
    expect(flow.phases[0].txs[0].tag).toBe("deposit_ft_transfer_call")
  })

  it("builds ft_transfer_call with correct args", async () => {
    const storageBalance: StorageBalance = { total: "2000000000000000000000", available: "0" }
    mockRpc.getVaultConfiguration.mockResolvedValue({ success: true, value: mockVaultConfig })
    mockRpc.getStorageBalance.mockResolvedValue({ success: true, value: storageBalance })

    const flow = await prepareDeposit(ctx, params)

    const depositTx = flow.phases[0].txs[0]
    expect(depositTx.receiverId).toBe("usdc.near")
    expect(depositTx.actions[0].type).toBe("FunctionCall")

    const action = depositTx.actions[0]
    if (action.type === "FunctionCall") {
      expect(action.params.methodName).toBe("ft_transfer_call")
      expect(action.params.args).toEqual({
        receiver_id: "vault.near",
        amount: "1000000",
        msg: DEPOSIT_MSG_SUPPLY,
      })
    }
  })

  it("adds warning when deposit exceeds max capacity", async () => {
    const storageBalance: StorageBalance = { total: "2000000000000000000000", available: "0" }
    mockRpc.getVaultConfiguration.mockResolvedValue({ success: true, value: mockVaultConfig })
    mockRpc.getStorageBalance.mockResolvedValue({ success: true, value: storageBalance })
    mockRpc.getMaxDeposit.mockResolvedValue({ success: true, value: "500000" })

    const flow = await prepareDeposit(ctx, { ...params, warnOnExcessDeposit: true })

    expect(flow.warnings).toBeDefined()
    expect(flow.warnings?.length).toBe(1)
    expect(flow.warnings?.[0]).toContain("exceeds vault max capacity")
  })

  it("throws on zero amount", async () => {
    mockRpc.getVaultConfiguration.mockResolvedValue({ success: true, value: mockVaultConfig })
    mockRpc.getStorageBalance.mockResolvedValue({ success: true, value: null })

    await expect(prepareDeposit(ctx, { ...params, amount: "0" })).rejects.toThrow(
      "Deposit amount must be greater than zero"
    )
  })
})
