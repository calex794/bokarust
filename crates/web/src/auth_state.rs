//! Per-process authentication runtime: database adapter, bounded Argon2 workers and login attempts.
use crate::auth_cookies::CookiePolicy;
use axum::http::StatusCode;
use bokarust_adapters::{
    account_security::session_token_digest,
    postgres_accounts::PgAccounts,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

const LOGIN_ATTEMPTS_PER_WINDOW: u32 = 5;
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
const MAX_LOGIN_KEYS: usize = 10_000;

struct AttemptWindow {
    count: u32,
    until: Instant,
}

#[derive(Clone)]
pub(crate) struct AuthState {
    pub(crate) accounts: PgAccounts,
    pub(crate) cookies: CookiePolicy,
    attempts: Arc<Mutex<HashMap<[u8; 32], AttemptWindow>>>,
    hash_slots: Arc<Semaphore>,
}

impl AuthState {
    pub(crate) fn new(accounts: PgAccounts, secure_cookies: bool) -> Self {
        Self {
            accounts,
            cookies: CookiePolicy { secure: secure_cookies },
            attempts: Arc::new(Mutex::new(HashMap::new())),
            hash_slots: Arc::new(Semaphore::new(4)),
        }
    }

    // Local brute-force bound; deployment across multiple replicas requires a shared limiter.
    pub(crate) fn allow_attempt(&self, email: &str) -> bool {
        let key = session_token_digest(email);
        let now = Instant::now();
        let mut attempts = self.attempts.lock().unwrap_or_else(|p| p.into_inner());
        attempts.retain(|_, item| item.until > now);
        if !attempts.contains_key(&key) && attempts.len() >= MAX_LOGIN_KEYS {
            return false;
        }
        let item = attempts.entry(key).or_insert(AttemptWindow {
            count: 0,
            until: now + LOGIN_WINDOW,
        });
        if item.count >= LOGIN_ATTEMPTS_PER_WINDOW {
            return false;
        }
        item.count += 1;
        true
    }

    pub(crate) fn forget_attempts(&self, email: &str) {
        let mut attempts = self.attempts.lock().unwrap_or_else(|p| p.into_inner());
        attempts.remove(&session_token_digest(email));
    }

    async fn password_job<T, F>(&self, job: F) -> Result<T, StatusCode>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = self
            .hash_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            job()
        })
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_rate_limit_is_bounded_and_resets_after_success() {
        let limiter = AuthState::new(PgAccounts::from_pool(sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://user:password@127.0.0.1:5432/not_used").unwrap()), true);
        for _ in 0..LOGIN_ATTEMPTS_PER_WINDOW {
            assert!(limiter.allow_attempt("alice@example.org"));
        }
        assert!(!limiter.allow_attempt("alice@example.org"));
        limiter.forget_attempts("alice@example.org");
        assert!(limiter.allow_attempt("alice@example.org"));
    }
}
