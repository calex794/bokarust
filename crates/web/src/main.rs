use bokarust_adapters::{in_memory::InMemoryCatalog, postgres::PgCatalog};
use sqlx::postgres::PgPoolOptions;
use std::{env, error::Error, net::SocketAddr, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind = env::var("BOKARUST_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let address: SocketAddr = bind.parse()?;

    let app = match env::var("DATABASE_URL") {
        Ok(database_url) => {
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .acquire_timeout(Duration::from_secs(5))
                .connect(&database_url)
                .await?;
            bokarust_adapters::MIGRATOR.run(&pool).await?;
            let secure_cookies = if env::var("BOKARUST_INSECURE_COOKIES").as_deref() == Ok("1") {
                if !address.ip().is_loopback() {
                    return Err("Insecure cookies are permitted only on loopback".into());
                }
                false
            } else {
                true
            };
            bokarust_web::router_with_auth(
                Arc::new(PgCatalog::new(pool.clone())),
                bokarust_adapters::postgres_accounts::PgAccounts::from_pool(pool),
                secure_cookies,
            )
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
            bokarust_web::router(Arc::new(InMemoryCatalog::default()))
        }
        Err(error) => return Err(error.into()),
    };

    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("BookaRust server listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}
