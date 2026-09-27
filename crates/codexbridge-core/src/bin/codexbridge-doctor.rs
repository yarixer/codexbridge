#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let workspace = std::env::args().nth(1).unwrap_or_else(|| ".".to_owned());
    let cursor_api_key = std::env::var("CURSOR_API_KEY").ok();
    let report =
        codexbridge_core::diagnostics::inspect(&workspace, cursor_api_key.as_deref(), None, None)
            .await;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
