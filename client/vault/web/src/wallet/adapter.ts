import type { NearActionV1, PlannedTxV1 } from "../types/flow.js"

export type WalletSelectorAction =
  | { type: "FunctionCall"; params: { methodName: string; args: Record<string, unknown>; gas: string; deposit: string } }
  | { type: "Transfer"; params: { deposit: string } }

export type WalletSelectorTransaction = {
  signerId: string
  receiverId: string
  actions: WalletSelectorAction[]
}

export type SignAndSendResult = {
  transactionHashes: string[]
}

export type WalletAdapter = {
  signAndSendTransactions(args: {
    transactions: WalletSelectorTransaction[]
  }): Promise<SignAndSendResult>
}

export function plannedTxToWalletTx(tx: PlannedTxV1): WalletSelectorTransaction {
  return {
    signerId: tx.signerId,
    receiverId: tx.receiverId,
    actions: tx.actions.map(actionToWalletAction),
  }
}

export function actionToWalletAction(action: NearActionV1): WalletSelectorAction {
  return action as WalletSelectorAction
}

export function plannedTxsToWalletTxs(txs: readonly PlannedTxV1[]): WalletSelectorTransaction[] {
  return txs.map(plannedTxToWalletTx)
}
