//! Infrastructure adapters. The in-memory catalog is for local development/tests.
#![forbid(unsafe_code)]

pub mod account_security;
pub mod in_memory;
pub mod postgres;
pub mod postgres_accounts;
pub mod postgres_author;
mod postgres_author_books;
mod postgres_author_chapters;
pub mod postgres_reading;
pub mod postgres_progress;

/// Shared versioned migrations for accounts, sessions, and books.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
