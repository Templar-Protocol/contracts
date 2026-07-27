//! Wall-clock benchmarks for the sandbox harness primitives — see
//! [`templar_gateway_testing::bench`]. Run it with `just bench-sandbox`.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    templar_gateway_testing::bench::run().await
}
