# Testing & Code Coverage

## Test Execution

Run the complete local suite through the same entrypoints used by CI:

```bash
just test
```

Use `just test-fast` for the complete non-node gate, including non-node
integration targets, or `just test-sandbox` for the node-backed gate. The sandbox recipe prebuilds NEAR contracts before
starting its pooled `neard` instances; pass `--stale` to reuse the contracts already built into `target/near`.

Run the artifact drift check separately when validating checked-in WASM blobs —
a pure hash/version check with no builds:

```bash
./script/check-artifact-drift.sh
```

## Local Testing

### Running Tests with Coverage

```bash
# Generate and open HTML coverage for the fast library-test cut
just coverage

# Generate coverage.lcov
just coverage-lcov
```

### Test Categories

- **Unit tests**: Module-level functionality
- **Integration tests**: Cross-module interactions
- **Contract tests**: Smart contract behavior
- **End-to-end tests**: Full workflow validation

## Performance Testing

### Gas Usage Analysis

Gas usage analysis is available through existing tools:

```bash
./script/gas-report.sh
```

This generates a gas report for market operations, including average gas costs for individual operations and snapshot iteration limits.

### Test-Gate Timing

The node gate is the slowest thing we run, so its cost is measured rather than
guessed. Two tools:

```bash
# Time the harness primitives on a dedicated neard: block-latency floor,
# per-transaction and per-patch costs, fixture setup.
just bench-sandbox

# Compare two whole-suite runs. `just test-sandbox` writes per-test timings to
# target/nextest/sandbox/junit.xml (see [profile.sandbox.junit] in
# .config/nextest.toml); copy it aside before and after a change.
./script/bench/junit-diff.py before.xml after.xml
```

`junit-diff.py` totals are summed per-test durations. The gate runs several
tests concurrently, so those intervals overlap: the totals measure work done,
not gate wall clock. Its per-test breakdown is the point — a change that halves
the suite can still make one test much worse.

What the measurements established, so it need not be re-derived:

- **Node round-trips dominate; WASM payload is ~free.** 570KB of extra contract
  code adds ~18ms to a deploy. Installing contract code via `sandbox_patch_state`
  works but is *not* faster, and it forfeits batching deploy+init into one
  transaction. Same verdict rules out global contracts. Don't retry either.
- **`sandbox_patch_state` costs ~200ms per _call_, not per record.** Minting N
  accounts in one patch therefore costs roughly what minting one does — hence the
  harness's batched `create_accounts`.
- **Block production delay is the other lever, and it is local-only.** See the
  sandbox cadence note below.

### Sandbox Block Cadence

Locally the harness runs `neard` at a 40ms `min_block_production_delay` instead
of the stock 120ms, which roughly halves the node gate. **CI pins the stock 120ms**
(`NEAR_SANDBOX_BLOCK_MS` in `.github/workflows/test.yml`): a 4-vCPU runner cannot
sustain four nodes producing blocks 3× as often, and attempting it caused
widespread transaction-finality failures. Reducing parallelism to compensate was
measured and is *slower* than stock.

Override the cadence with `NEAR_SANDBOX_BLOCK_MS=<ms>`. `max_block_production_delay`
is compensated in the opposite direction so that `avg(min, max)` stays fixed at
310ms — nearcore credits a `sandbox_fast_forward` as
`delta_height × avg(min, max)`, so holding that average keeps every
time-sensitive test's simulated time unchanged whatever the real cadence.

Because local blocks are faster than CI's, a test that leans on *incidental*
block cadence to cross a time boundary will pass in one place and fail in the
other. Advance chain time explicitly with `fast_forward` rather than relying on
how long some operations happen to take.
