#[tokio::main]
async fn main() -> anyhow::Result<()> {
    quarry_cli::run().await
}
