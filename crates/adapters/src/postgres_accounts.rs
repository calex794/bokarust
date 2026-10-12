//! PostgreSQL persistence for identities and revocable session lookups.
//! This adapter never stores plaintext passwords or session tokens.
#![forbid(unsafe_code)]

use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::{num::NonZeroU32, time::Duration};

#[derive(Clone)]
pub struct PgAccounts {
    pool: PgPool,
}

pub struct StoredUser {
    pub id: i64,
    pub email_normalized: String,
    pub display_name: String,
    pub password_hash: String,
}

impl PgAccounts {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Invoke explicitly before accepting traffic. Migrations are versioned.
    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        crate::MIGRATOR.run(&self.pool).await
    }

    /// A concurrent duplicate is rejected by PostgreSQL's unique constraint.
    pub async fn insert_user(
        &self,
        normalized_email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO users (email_normalized, display_name, password_hash) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(normalized_email)
        .bind(display_name)
        .bind(password_hash)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn find_user(
        &self,
        normalized_email: &str,
    ) -> Result<Option<StoredUser>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, email_normalized, display_name, password_hash \
             FROM users WHERE email_normalized = $1",
        )
        .bind(normalized_email)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(StoredUser {
                id: r.try_get("id")?,
                email_normalized: r.try_get("email_normalized")?,
                display_name: r.try_get("display_name")?,
                password_hash: r.try_get("password_hash")?,
            })
        })
        .transpose()
    }

    /// Accepts a SHA-256 token digest, never the raw token; expiry uses DB time.
    pub async fn insert_session(
        &self,
        user_id: i64,
        digest: &[u8; 32],
        ttl_seconds: NonZeroU32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO sessions (user_id, token_hash, expires_at) \
             VALUES ($1, $2, now() + ($3::double precision * INTERVAL '1 second'))",
        )
        .bind(user_id)
        .bind(digest.as_slice())
        .bind(ttl_seconds.get() as f64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn active_session_user(&self, digest: &[u8; 32]) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT user_id FROM sessions \
             WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(digest.as_slice())
        .fetch_optional(&self.pool)
        .await
    }

    /// Register and establish the first session atomically. A failed session
    /// write rolls back user creation, avoiding a stranded registration.
    pub async fn register_with_session(
        &self,
        email: &str,
        name: &str,
        password_hash: &str,
        digest: &[u8; 32],
        ttl: NonZeroU32,
    ) -> Result<i64, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (email_normalized, display_name, password_hash) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO sessions (user_id, token_hash, expires_at) \
             VALUES ($1, $2, now() + ($3::double precision * INTERVAL '1 second'))",
        )
        .bind(user_id)
        .bind(digest.as_slice())
        .bind(ttl.get() as f64)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(user_id)
    }

    /// Read the minimal private profile, never return password hashes to HTML.
    pub async fn user_display_name(&self, user_id: i64) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Atomic counter prevents concurrent attempts bypassing limits.
    /// Counters are shared by all server processes; the caller hashes keys.
    pub async fn permit_auth_attempt(
        &self,
        key: &[u8; 32],
        max_attempts: i32,
        window_seconds: i32,
    ) -> Result<bool, sqlx::Error> {
        let attempts: i32 = sqlx::query_scalar(
            "INSERT INTO auth_attempts AS rate (key_hash, attempts, period_started) \
             VALUES ($1, 1, now()) ON CONFLICT (key_hash) DO UPDATE SET \
             attempts = CASE WHEN rate.period_started + ($2::integer * INTERVAL '1 second') <= now() \
                  THEN 1 ELSE rate.attempts + 1 END, \
             period_started = CASE WHEN rate.period_started + ($2::integer * INTERVAL '1 second') <= now() \
                  THEN now() ELSE rate.period_started END \
             RETURNING attempts",
        )
        .bind(key.as_slice())
        .bind(window_seconds)
        .fetch_one(&self.pool)
        .await?;
        Ok(attempts <= max_attempts)
    }

    /// Successful login resets the per-identity attempt counter only.
    pub async fn clear_auth_attempt(&self, key: &[u8; 32]) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM auth_attempts WHERE key_hash = $1")
            .bind(key.as_slice())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Safe to retry; session lookup must reject the same token immediately.
    pub async fn revoke_session(&self, digest: &[u8; 32]) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE sessions SET revoked_at = now() \
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(digest.as_slice())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() != 0)
    }
}
