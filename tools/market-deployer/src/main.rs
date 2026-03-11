#[tokio::main]
async fn main() -> anyhow::Result<()> {
    market_deployer::run().await
}
