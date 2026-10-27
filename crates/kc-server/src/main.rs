//! karucode server binary.
//!
//! All logic lives in the `kc_server` library so it can be tested and embedded;
//! this file only wires the process entry point.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    kc_server::run().await
}
