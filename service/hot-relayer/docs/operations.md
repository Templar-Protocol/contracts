# HOT Relayer Operations

## Deployment Checklist

1. Confirm the counterparty vault, Stellar receiver, HOT chain id, and HOT token id with the
   deployment record.
2. Generate a fresh `HOT_RELAYER_AUTH_TOKEN` and store it in the runtime secret manager.
3. Configure `HOT_MPC_API_URL` without embedding credentials in the URL when possible.
4. Set `HOT_RELAYER_MPC_TIMEOUT_SECS` lower than the upstream request timeout for the load balancer
   or job runner.
5. Set `HOT_RELAYER_MAX_REQUEST_BYTES` to the smallest payload size that still covers expected HOT
   completion requests.
6. Run the service in a network segment reachable only by the trusted caller that completes HOT
   deposits or withdrawals.
7. Verify `/health` and `/metrics` before routing relay traffic.
8. Send a bad unauthenticated request to both relay endpoints and confirm HTTP 401.
9. Send an authenticated request with a wrong chain, token, or receiver and confirm HTTP 400 before
   enabling production traffic.

## Runtime Model

The HOT relayer is intentionally narrow. It exposes only:

- `GET /health`
- `GET /metrics`
- `POST /relay/deposit/complete`
- `POST /relay/withdrawal/complete`

Relay routes require `Authorization: Bearer $HOT_RELAYER_AUTH_TOKEN`. The service validates the
configured HOT routing at startup and validates every request's chain id, token id, receiver,
nonce, and amount before it calls the HOT MPC API. It does not load treasury keys, local chain
signing keys, or unrelated EVM, Solana, Stellar, or NEAR treasury handlers.

The service handles SIGINT and SIGTERM through graceful Axum shutdown. It has no local database or
run lock because each request is stateless after validation. Deploy multiple replicas only if the
HOT MPC backend and caller flow can tolerate duplicate requests.

## Configuration

Required:

- `HOT_MPC_API_URL`
- `HOT_RELAYER_NEAR_RECEIVER`
- `HOT_RELAYER_STELLAR_RECEIVER`
- `HOT_RELAYER_TOKEN_ID`
- `HOT_RELAYER_AUTH_TOKEN`

Optional:

- `PORT`, default `3001`
- `HOT_RELAYER_CHAIN_ID`, default `1100`
- `HOT_RELAYER_MPC_TIMEOUT_SECS`, default `10`
- `HOT_RELAYER_MAX_REQUEST_BYTES`, default `16384`
- `RUST_LOG`, recommended `info,hot_relayer=debug,tower_http=info`

Startup logs redact userinfo, path, query strings, and fragments from `HOT_MPC_API_URL`.

## Build

Build the runtime image from the contracts repository root:

```sh
docker build -f service/hot-relayer/Dockerfile -t hot-relayer .
```

The Dockerfile uses `cargo build --locked`, so image builds fail if `Cargo.toml` and `Cargo.lock`
drift.

For a direct local run:

```sh
cargo run -p templar-hot-relayer
```

## Monitoring

Alert on:

- HTTP 401 spikes on relay endpoints,
- HTTP 400 spikes from invalid chain, token, receiver, nonce, or amount,
- HOT MPC HTTP failures or non-200 responses,
- request latency near `HOT_RELAYER_MPC_TIMEOUT_SECS`,
- `/health` failures,
- missing `/metrics` scrapes,
- unexpected configuration changes to receiver, token id, chain id, auth token, or MPC URL.

Keep dependency-level `debug` or `trace` logging disabled when upstream URLs might contain
credentials. The service redacts URLs in its own startup logs, but lower-level HTTP libraries can
include raw transport details in verbose spans.

## Incident Response

If the auth token is suspected to be exposed:

1. Remove external access to the service.
2. Rotate `HOT_RELAYER_AUTH_TOKEN`.
3. Restart every replica.
4. Confirm old-token requests return HTTP 401.
5. Re-enable trusted caller traffic.

If the MPC API is returning invalid or unexpected signatures, stop relay traffic before restarting.
This service intentionally does not hold treasury authority, so containment should focus on route
access, MPC API access, and the counterparty contract state.
