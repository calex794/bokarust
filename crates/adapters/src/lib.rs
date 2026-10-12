//! Infrastructure adapters. The in-memory catalog is for local development/tests.
#![forbid(unsafe_code)]

pub mod account_security;
pub mod in_memory;
pub mod postgres_accounts;
