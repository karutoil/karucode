//! karucode server binary.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    kc_server::run().await
}
