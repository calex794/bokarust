//! Apply user and session migrations once before enabling authentication.
use bokarust_adapters::postgres_accounts::PgAccounts;
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("DATABASE_URL")?;
    let accounts = PgAccounts::connect(&url).await?;
    accounts.migrate().await?;
    println!("PostgreSQL account migrations applied.");
    Ok(())
}
