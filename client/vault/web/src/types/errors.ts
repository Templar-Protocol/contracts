/**
 * Typed errors for vault web client.
 * These map common contract panics and execution failures to UX-friendly errors.
 */

export type VaultWebErrorCode =
  | "REFRESH_THROTTLED"
  | "INSUFFICIENT_STORAGE_DEPOSIT"
  | "INVALID_DEPOSIT_MSG"
  | "WRONG_UNDERLYING_TOKEN"
  | "TX_FAILED_FINAL"
  | "RPC_TIMEOUT"
  | "RPC_ERROR"
  | "WALLET_REJECTED"
  | "POLICY_VIOLATION"
  | "UNKNOWN"

export class VaultWebError extends Error {
  readonly code: VaultWebErrorCode
  readonly context?: ErrorContext

  constructor(code: VaultWebErrorCode, message: string, context?: ErrorContext) {
    super(message)
    this.name = "VaultWebError"
    this.code = code
    this.context = context
  }
}

export type ErrorContext = {
  phaseLabel?: string
  txTag?: string
  txHash?: string
  raw?: unknown
  required?: string
  attached?: string
}

/**
 * Parse a contract panic string and return a typed error if recognized.
 */
export function parseContractError(failureMessage: string, context?: Partial<ErrorContext>): VaultWebError {
  const msg = failureMessage.toLowerCase()

  // "Refresh throttled"
  if (msg.includes("refresh throttled")) {
    return new VaultWebError(
      "REFRESH_THROTTLED",
      "Market refresh is throttled. Please wait before trying again.",
      context
    )
  }

  // "Insufficient storage deposit for withdrawal request: required X, attached Y"
  const storageMatch = failureMessage.match(
    /Insufficient storage deposit for (\w+): required (\d+), attached (\d+)/i
  )
  if (storageMatch) {
    return new VaultWebError(
      "INSUFFICIENT_STORAGE_DEPOSIT",
      `Insufficient storage deposit: required ${storageMatch[2]} yoctoNEAR, attached ${storageMatch[3]} yoctoNEAR`,
      {
        ...context,
        required: storageMatch[2],
        attached: storageMatch[3],
      }
    )
  }

  // "Invalid deposit msg"
  if (msg.includes("invalid deposit msg")) {
    return new VaultWebError(
      "INVALID_DEPOSIT_MSG",
      "Invalid deposit message format. This is likely a bug in the client library.",
      context
    )
  }

  // "Invalid token ID" (wrong underlying token)
  if (msg.includes("invalid token id")) {
    return new VaultWebError(
      "WRONG_UNDERLYING_TOKEN",
      "The token sent does not match the vault's underlying asset.",
      context
    )
  }

  // Generic execution failure
  return new VaultWebError(
    "TX_FAILED_FINAL",
    `Transaction failed: ${failureMessage}`,
    { ...context, raw: failureMessage }
  )
}

/**
 * Create an RPC timeout error.
 */
export function rpcTimeoutError(phaseLabel: string, txHash?: string): VaultWebError {
  return new VaultWebError(
    "RPC_TIMEOUT",
    `Timed out waiting for transaction finality in phase "${phaseLabel}"`,
    { phaseLabel, txHash }
  )
}

/**
 * Create an RPC communication error.
 */
export function rpcError(message: string, raw?: unknown): VaultWebError {
  return new VaultWebError(
    "RPC_ERROR",
    `RPC error: ${message}`,
    { raw }
  )
}

/**
 * Create a wallet rejection error.
 */
export function walletRejectedError(message?: string): VaultWebError {
  return new VaultWebError(
    "WALLET_REJECTED",
    message ?? "Transaction was rejected by the wallet",
    {}
  )
}

/**
 * Create a policy violation error (local validation).
 */
export function policyViolationError(message: string): VaultWebError {
  return new VaultWebError(
    "POLICY_VIOLATION",
    message,
    {}
  )
}
