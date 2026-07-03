mod gateway_cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    gateway_cli::run().await
}
