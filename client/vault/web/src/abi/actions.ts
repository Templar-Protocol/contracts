import type { FunctionCallActionV1 } from "../types/flow.js"
import type { MarketId, FungibleAsset } from "../abi/generated/types.js"
import type { ResolvedPolicy } from "../policy/defaults.js"

export const DEPOSIT_MSG_SUPPLY = '\"Supply\"'

export function buildStorageDepositAction(
  policy: ResolvedPolicy,
  accountId?: string,
  registrationOnly?: boolean
): FunctionCallActionV1 {
  const args: Record<string, unknown> = {}
  if (accountId !== undefined) {
    args.account_id = accountId
  }
  if (registrationOnly !== undefined) {
    args.registration_only = registrationOnly
  }

  return {
    type: "FunctionCall",
    params: {
      methodName: "storage_deposit",
      args,
      gas: policy.gas.storage_deposit,
      deposit: policy.storage.share_storage_deposit_yocto,
    },
  }
}

export function buildWithdrawAction(
  policy: ResolvedPolicy,
  amount: string,
  receiverId: string
): FunctionCallActionV1 {
  return {
    type: "FunctionCall",
    params: {
      methodName: "withdraw",
      args: {
        amount,
        receiver: receiverId,
      },
      gas: policy.gas.withdraw,
      deposit: policy.storage.withdraw_request_yocto,
    },
  }
}

export function buildRefreshMarketsAction(
  policy: ResolvedPolicy,
  markets: readonly MarketId[]
): FunctionCallActionV1 {
  return {
    type: "FunctionCall",
    params: {
      methodName: "refresh_markets",
      args: {
        markets: [...markets],
      },
      gas: policy.gas.refresh_markets,
      deposit: "0",
    },
  }
}

export function buildFtTransferCallAction(
  policy: ResolvedPolicy,
  receiverId: string,
  amount: string,
  msg: string
): FunctionCallActionV1 {
  return {
    type: "FunctionCall",
    params: {
      methodName: "ft_transfer_call",
      args: {
        receiver_id: receiverId,
        amount,
        msg,
      },
      gas: policy.gas.ft_transfer_call,
      deposit: policy.ft.ft_transfer_call_attached_yocto,
    },
  }
}

export function getUnderlyingTokenContractId(token: FungibleAsset): string {
  if ("Nep141" in token) {
    return token.Nep141
  }
  if ("Nep245" in token) {
    return token.Nep245.contract_id
  }
  throw new Error("Unknown underlying token type")
}
