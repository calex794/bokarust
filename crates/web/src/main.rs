use bokarust_adapters::{in_memory::InMemoryCatalog, postgres::PgCatalog};
use bokarust_application::catalog::PublicBookRepository;
use sqlx::postgres::PgPoolOptions;
use std::{env, error::Error, net::SocketAddr, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind = env::var("BOKARUST_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let address: SocketAddr = bind.parse()?;

    let catalog: Arc<dyn PublicBookRepository> = match env::var("DATABASE_URL") {
        Ok(database_url) => {
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .acquire_timeout(Duration::from_secs(5))
                .connect(&database_url)
                .await?;
            bokarust_adapters::MIGRATOR.run(&pool).await?;
            Arc::new(PgCatalog::new(pool))
        }
        Err(env::VarError::NotPresent) => {
            if env::var("BOKARUST_DEMO_IN_MEMORY").as_deref() != Ok("1") {
                return Err(
                    "Set DATABASE_URL to use PostgreSQL (or BOKARUST_DEMO_IN_MEMORY=1 for a loopback-only demo)"
                        .into(),
                );
            }
            if !address.ip().is_loopback() {
                return Err("In-memory demo cannot listen on a public interface".into());
            }
            Arc::new(InMemoryCatalog::default())
        }
        Err(error) => return Err(error.into()),
    };

    let listener = tokio::net::TcpListener::bind(address).await?;
    let app = bokarust_web::router(catalog);
    println!("BookaRust server listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}
