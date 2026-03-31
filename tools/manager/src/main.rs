#[tokio::main]
async fn main() -> anyhow::Result<()> {
    templar_manager::run().await
}
