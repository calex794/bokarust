use bokarust_adapters::{
    in_memory::InMemoryCatalog, postgres::PgCatalog, postgres_accounts::PgAccounts,
    postgres_reading::PgReader,
};
use bokarust_application::{catalog::PublicBookRepository, reading::PublicReadingRepository};
use sqlx::postgres::PgPoolOptions;
use std::{env, error::Error, net::SocketAddr, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind = env::var("BOKARUST_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let address: SocketAddr = bind.parse()?;

    let (catalog, accounts, reader): (
        Arc<dyn PublicBookRepository>,
        Option<PgAccounts>,
        Option<Arc<dyn PublicReadingRepository>>,
    ) = match env::var("DATABASE_URL") {
        Ok(database_url) => {
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .acquire_timeout(Duration::from_secs(5))
                .connect(&database_url)
                .await?;
            bokarust_adapters::MIGRATOR.run(&pool).await?;
            (
                Arc::new(PgCatalog::new(pool.clone())),
                Some(PgAccounts::from_pool(pool.clone())),
                Some(Arc::new(PgReader::new(pool))),
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
            (Arc::new(InMemoryCatalog::default()), None, None)
        }
        Err(error) => return Err(error.into()),
    };

    // The only insecure-cookie mode is explicit and restricted to loopback.
    let insecure = env::var("BOKARUST_INSECURE_COOKIES").as_deref() == Ok("1");
    if insecure && !address.ip().is_loopback() {
        return Err("Insecure cookies may be used only on loopback".into());
    }
    let public_origin =
        env::var("BOKARUST_PUBLIC_ORIGIN").unwrap_or_else(|_| format!("http://{address}"));
    if accounts.is_some() && !address.ip().is_loopback() && !public_origin.starts_with("https://") {
        return Err("Public binding requires BOKARUST_PUBLIC_ORIGIN with HTTPS".into());
    }
    let app = match accounts {
        Some(accounts) => {
            let reader = reader.expect("PostgreSQL reader accompanies PostgreSQL accounts");
            bokarust_web::router_with_auth_and_reading(
                catalog, reader, accounts, !insecure, public_origin,
            )
        }
        None => bokarust_web::router(catalog),
    };
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("BookaRust server listening on http://{address}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
