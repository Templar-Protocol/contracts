# Migrating from the NEAR Intents bridge

If your integration still calls the NEAR Intents Bridge JSON-RPC API, switch to Templar's backend HTTP API.

**Do not call `chaindefuser.com` directly.** The public Bridge API at `https://bridge.chaindefuser.com/rpc` is closing; the NEAR Intents changelog dates that closure to 11 November 2026 and points new work at the 1-Click Swap API. Templar's backend handles the upstream connection. Partners do not need an IP allowlist.

Companion OpenAPI documentation for Bridge routes, parameters, and schemas is maintained at:

**[https://api.templarfi.org/docs?spec=bridge](https://api.templarfi.org/docs?spec=bridge)**

The server base URL is `https://api.templarfi.org/v1`.

## Authentication

This matches the live OpenAPI documents (Bridge 1.6.0 and the gateway description):

- `GET` routes are public. No API key and no IP allowlist are required.
- `POST` routes require the `X-API-Key` header.
- If you send an `X-API-Key` on a public `GET` and the key is invalid or revoked, the gateway rejects the request rather than treating it as keyless.
- Keyless `GET`s are rate-limited per client IP. A valid key is limited at that key's tier instead. Only `GET /health` is exempt from the rate limit.

## JSON-RPC methods to HTTP routes

Method names are the upstream Intents JSON-RPC methods that Templar's backend still calls. HTTP paths come from the live Bridge OpenAPI and are prefixed by the documented base URL `/v1`. Several OpenAPI operation descriptions name the upstream method they proxy.

| Legacy JSON-RPC method | Templar HTTP route | Notes |
|---|---|---|
| `supported_tokens` | `GET /v1/bridge/tokens` | Public. Optional `chains` query parameter. |
| `deposit_address` | `GET /v1/bridge/deposit/address` | Public. Thin proxy of upstream `deposit_address`. |
| `notify_deposit` | `POST /v1/bridge/deposit/confirm` | Requires `X-API-Key`. Forwards optional `chain` and `memo` to upstream `notify_deposit`. |
| `recent_deposits` | `GET /v1/bridge/deposit/status` | Public. Returns recent deposits for an account and chain. |
| `withdrawal_estimate` | `GET /v1/bridge/withdraw/estimate` | Public. Thin proxy of upstream `withdrawal_estimate`. |
| `recent_withdrawals` | `GET /v1/bridge/withdrawals/recent` | Public. Thin proxy of upstream `recent_withdrawals`. |
| `withdrawal_status` | `GET /v1/bridge/withdraw/status` | Public. Thin proxy of upstream `withdrawal_status`. |

These two routes are Templar-composed. They are not one-to-one JSON-RPC proxies. They call the methods above internally and also build ready-to-sign transactions:

| Templar HTTP route | Auth | What it uses upstream |
|---|---|---|
| `POST /v1/bridge/deposit/prepare` | `X-API-Key` | `deposit_address`, plus transaction construction for the token transfer |
| `POST /v1/bridge/withdraw/prepare` | `X-API-Key` | `withdrawal_estimate`, plus a ready-to-sign NEAR `ft_withdraw` transaction |

Do not invent additional `/v1/bridge/*` paths from the JSON-RPC method list. If a method is not in the tables above, it is not exposed as a documented Templar HTTP route.

For cross-chain token *swaps* rather than PoA deposits and withdrawals, use the Swaps API (`/v1/swaps/oneclick/*` and `/v1/swaps/stellar/*`). See [API Reference](./api-reference.md) and [https://api.templarfi.org/docs?spec=swaps](https://api.templarfi.org/docs?spec=swaps). The pre-1.6.0 aliases `/swaps/quote` and `/swaps/status` were removed in Swaps 2.0.0.
