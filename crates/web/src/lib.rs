//! HTTP entry point. Route handlers are thin; decisions live in application/domain.
#![forbid(unsafe_code)]

mod auth;
mod auth_hashing;
mod auth_cookies;
mod pages;
mod routes;

pub use routes::{router, router_with_auth};
