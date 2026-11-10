# Testing & Code Coverage

## Test Execution

Run the complete local suite through the same entrypoints used by CI:

```bash
just test
```

Use `just test-fast` for the complete non-node gate, including non-node
integration targets, or `just test-sandbox` for the node-backed gate. The sandbox recipe prebuilds NEAR contracts before
starting its pooled `neard` instances; pass `--stale` to reuse the contracts already built into `target/near`.

Run the artifact drift check separately when validating the release catalog.
It may compile the Rust test crate, but does not build contract WASM:

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

The NEAR CI job has a 120-minute ceiling to include cold builds and the long
real-script migration acceptance matrix. This is a job budget, not a relaxed
protocol-validity or finality check. The sandbox profile still terminates an
individual test after five 120-second slow periods.

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

### Sandbox State Patches

The workspace pins `near-sandbox = "=0.3.11"` so pooled nodes, owned harnesses,
and native replay all launch nearcore **2.12.0**. This is the first stable release
containing [the commit-aware patch tracker](https://github.com/near/nearcore/pull/15534).
Nearcore 2.11.0 and 2.11.1 can permanently discard a patch during block
preprocessing or an old-chunk transition while acknowledging its consumption.
Polling longer cannot recover a discarded patch; do not compensate by resending
records or relaxing finalized-state checks.

The pinned RPC waits for committed state, not finalized visibility. Continue
waiting for the patched key or exact metadata to reach `Final` before signing
or capturing a replay checkpoint.

### Historical Transaction Outcomes

A long campaign re-reads its original finalized transaction outcomes on restart.
Default fast-init sandbox history can be pruned before that workflow finishes:
the same hash then becomes `UnknownTransaction`, and waiting for `FINAL` may time
out. Neither response authorizes a replacement transaction.

The dedicated released-market fixtures start through
`SandboxHarness::start_owned_with_config`, preserving `sandbox_config()` and
setting the **flat** `additional_config["gc_num_epochs_to_keep"]` to 1,000.
Fast-init epochs are 60 blocks, so this retains 60,000 blocks for bounded fixture
replay without changing consensus timing, expiry or receipt checks. Normal
harness and pooled-node defaults remain unchanged. Nearcore's GC fields are
[serde-flattened](https://github.com/near/nearcore/blob/2.12.0/nearcore/src/config.rs#L318-L320);
a nested `gc` object does not configure this retention.

Configure sufficient history before execution. Increasing retention later cannot
restore records already pruned.


### Settled Account Checkpoints

The single-submission startup, metadata-reset, raw-value and code-preservation
regression is `just test-sandbox -p templar-gateway-testing --test sandbox_patch`.

`SandboxHarness` uses `ExecutedOptimistic` for ordinary setup transactions; that
status does not include every gas refund. Before comparing complete account
checkpoints, wait for the relevant transaction with `tx.get` and
`wait_until: "FINAL"`, and verify its downstream receipts. A fixed number of
`fast_forward` blocks is a time advance, not that transaction's settlement proof.

`sandbox_patch_state` now acknowledges committed state, not finalized visibility. After
restoring replay account metadata, wait until a `FINAL` account view matches the
exact amount, locked balance, storage usage, and local/global code linkage.
Finalize staging transactions and their refunds before that metadata reset;
an old access key reaching finality does not fence a later account patch.

Keep native amount, locked balance, storage usage, code/global identity,
effective code bytes, access keys/nonces, and raw storage comparisons separate
so a failure identifies the changed component. Do not drop a component merely
because asynchronous setup makes a composite digest differ.

The configuration-patch sandbox gate also submits an intentionally failing
deploy→patch→restore batch directly through the native gateway. Its guard fails
after the generated `STATE` write: the failed receipt must leave original
storage, configuration, effective code, and local/global linkage intact.
Gas balance and signer nonce are not rollback invariants.
