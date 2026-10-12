//! Bound concurrent Argon2 work to avoid overloading the blocking executor.
#![forbid(unsafe_code)]

use axum::http::StatusCode;
use std::sync::Arc;
use tokio::sync::Semaphore;

const MAX_PASSWORD_JOBS: usize = 4;

#[derive(Clone)]
pub(crate) struct PasswordWork {
    slots: Arc<Semaphore>,
}

impl PasswordWork {
    pub(crate) fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_PASSWORD_JOBS)),
        }
    }

    pub(crate) async fn run<T, F>(&self, job: F) -> Result<T, StatusCode>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;

        // Keep the permit in the blocking worker even if the HTTP request is cancelled.
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

    #[tokio::test]
    async fn full_capacity_rejects_immediately_and_recovers() {
        let work = PasswordWork::new();
        let held: Vec<_> = (0..MAX_PASSWORD_JOBS)
            .map(|_| work.slots.clone().try_acquire_owned().expect("free slot"))
            .collect();

        assert_eq!(
            work.run(|| true).await,
            Err(StatusCode::TOO_MANY_REQUESTS)
        );

        drop(held);
        assert_eq!(work.run(|| true).await, Ok(true));
    }
}
