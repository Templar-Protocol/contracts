import { describe, it, expect, vi, beforeEach } from "vitest"
import { executeFlow } from "./execute.js"
import type { PreparedFlowV1 } from "../types/flow.js"
import type { VaultWebContext } from "../types/context.js"
import type { WalletAdapter } from "../wallet/adapter.js"
import { NearRpcClient } from "../rpc/client.js"

vi.mock("../rpc/client.js", () => ({
  NearRpcClient: vi.fn(),
}))

describe("executeFlow", () => {
  const ctx: VaultWebContext = {
    networkId: "testnet",
    vaultContractId: "vault.near",
    rpcUrl: "https://rpc.testnet.near.org",
  }

  let mockWallet: WalletAdapter
  let mockRpc: { waitForFinality: ReturnType<typeof vi.fn> }

  beforeEach(() => {
    mockWallet = {
      signAndSendTransactions: vi.fn(),
    }
    mockRpc = {
      waitForFinality: vi.fn(),
    }
    vi.mocked(NearRpcClient).mockImplementation(() => mockRpc as unknown as NearRpcClient)
  })

  const singlePhaseFlow: PreparedFlowV1 = {
    version: 1,
    label: "Test",
    phases: [
      {
        label: "Phase 1",
        barrier: "finalized",
        batchable: false,
        txs: [
          {
            signerId: "user.near",
            receiverId: "vault.near",
            actions: [
              {
                type: "FunctionCall",
                params: { methodName: "test", args: {}, gas: "100", deposit: "0" },
              },
            ],
            tag: "test",
          },
        ],
      },
    ],
  }

  it("executes single phase successfully", async () => {
    vi.mocked(mockWallet.signAndSendTransactions).mockResolvedValue({
      transactionHashes: ["hash1"],
    })
    mockRpc.waitForFinality.mockResolvedValue({ status: "success", txHash: "hash1" })

    const result = await executeFlow(ctx, singlePhaseFlow, mockWallet)

    expect(result.success).toBe(true)
    expect(result.phaseResults).toHaveLength(1)
    expect(result.phaseResults[0].txResults[0].success).toBe(true)
    expect(result.phaseResults[0].txResults[0].txHash).toBe("hash1")
  })

  it("stops on tx failure and returns error", async () => {
    vi.mocked(mockWallet.signAndSendTransactions).mockResolvedValue({
      transactionHashes: ["hash1"],
    })
    mockRpc.waitForFinality.mockResolvedValue({
      status: "failure",
      txHash: "hash1",
      failureReason: "Refresh throttled",
    })

    const result = await executeFlow(ctx, singlePhaseFlow, mockWallet)

    expect(result.success).toBe(false)
    expect(result.error?.code).toBe("REFRESH_THROTTLED")
    expect(result.phaseResults[0].txResults[0].success).toBe(false)
  })

  it("handles wallet rejection", async () => {
    vi.mocked(mockWallet.signAndSendTransactions).mockRejectedValue(
      new Error("User rejected the transaction")
    )

    const result = await executeFlow(ctx, singlePhaseFlow, mockWallet)

    expect(result.success).toBe(false)
    expect(result.phaseResults[0].txResults[0].success).toBe(false)
    expect(result.phaseResults[0].txResults[0].failureReason).toContain("rejected")
  })

  it("executes multi-phase flow in order", async () => {
    const multiPhaseFlow: PreparedFlowV1 = {
      version: 1,
      label: "Multi",
      phases: [
        {
          label: "Phase 1",
          barrier: "finalized",
          batchable: false,
          txs: [
            {
              signerId: "user.near",
              receiverId: "vault.near",
              actions: [{ type: "FunctionCall", params: { methodName: "p1", args: {}, gas: "100", deposit: "0" } }],
              tag: "phase1",
            },
          ],
        },
        {
          label: "Phase 2",
          barrier: "finalized",
          batchable: false,
          txs: [
            {
              signerId: "user.near",
              receiverId: "vault.near",
              actions: [{ type: "FunctionCall", params: { methodName: "p2", args: {}, gas: "100", deposit: "0" } }],
              tag: "phase2",
            },
          ],
        },
      ],
    }

    vi.mocked(mockWallet.signAndSendTransactions)
      .mockResolvedValueOnce({ transactionHashes: ["hash1"] })
      .mockResolvedValueOnce({ transactionHashes: ["hash2"] })

    mockRpc.waitForFinality
      .mockResolvedValueOnce({ status: "success", txHash: "hash1" })
      .mockResolvedValueOnce({ status: "success", txHash: "hash2" })

    const result = await executeFlow(ctx, multiPhaseFlow, mockWallet)

    expect(result.success).toBe(true)
    expect(result.phaseResults).toHaveLength(2)
    expect(mockWallet.signAndSendTransactions).toHaveBeenCalledTimes(2)
    expect(mockRpc.waitForFinality).toHaveBeenCalledTimes(2)
  })

  it("stops at first failing phase and does not continue", async () => {
    const multiPhaseFlow: PreparedFlowV1 = {
      version: 1,
      label: "Multi",
      phases: [
        {
          label: "Phase 1",
          barrier: "finalized",
          batchable: false,
          txs: [
            {
              signerId: "user.near",
              receiverId: "vault.near",
              actions: [{ type: "FunctionCall", params: { methodName: "p1", args: {}, gas: "100", deposit: "0" } }],
              tag: "phase1",
            },
          ],
        },
        {
          label: "Phase 2",
          barrier: "finalized",
          batchable: false,
          txs: [
            {
              signerId: "user.near",
              receiverId: "vault.near",
              actions: [{ type: "FunctionCall", params: { methodName: "p2", args: {}, gas: "100", deposit: "0" } }],
              tag: "phase2",
            },
          ],
        },
      ],
    }

    vi.mocked(mockWallet.signAndSendTransactions).mockResolvedValueOnce({
      transactionHashes: ["hash1"],
    })

    mockRpc.waitForFinality.mockResolvedValueOnce({
      status: "failure",
      txHash: "hash1",
      failureReason: "Some error",
    })

    const result = await executeFlow(ctx, multiPhaseFlow, mockWallet)

    expect(result.success).toBe(false)
    expect(result.phaseResults).toHaveLength(1)
    expect(mockWallet.signAndSendTransactions).toHaveBeenCalledTimes(1)
  })
})
