# HOT Relayer

Narrow HOT bridge completion service. It exposes only:

- `GET /health`
- `GET /metrics`
- `POST /relay/deposit/complete`
- `POST /relay/withdrawal/complete`

The service requires bearer auth and validates HOT chain, token, receiver, nonce, and amount before
calling the configured HOT MPC API. It does not load treasury keys or initialize unrelated chain
handlers.

## Configuration

```bash
export PORT=3001
export HOT_MPC_API_URL=https://your-hot-mpc-api
export HOT_RELAYER_NEAR_RECEIVER=vault-counterparty.near
export HOT_RELAYER_STELLAR_RECEIVER=GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
export HOT_RELAYER_TOKEN_ID=1100_CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
export HOT_RELAYER_CHAIN_ID=1100
export HOT_RELAYER_AUTH_TOKEN=replace-with-long-random-token
export HOT_RELAYER_MPC_TIMEOUT_SECS=10
export HOT_RELAYER_MAX_REQUEST_BYTES=16384
```

Run:

```bash
cargo run -p templar-hot-relayer
```

Build the production image from the contracts repository root:

```bash
docker build -f service/hot-relayer/Dockerfile -t hot-relayer .
```

See [docs/operations.md](docs/operations.md) for deployment, monitoring, and incident response
notes.
