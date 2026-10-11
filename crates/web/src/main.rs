use bokarust_adapters::in_memory::InMemoryCatalog;
use std::{env, error::Error, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Loopback only. The in-memory adapter is deliberately not production-ready.
    let bind = env::var("BOKARUST_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let app = bokarust_web::router(Arc::new(InMemoryCatalog::default()));
    println!("BookaRust dev server: http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}
