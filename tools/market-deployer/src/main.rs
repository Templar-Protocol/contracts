#[tokio::main]
async fn main() -> anyhow::Result<()> {
    templar_deployment_manager::run().await
}
