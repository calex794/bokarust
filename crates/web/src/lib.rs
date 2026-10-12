//! HTTP entry point. Route handlers are thin; decisions live in application/domain.
#![forbid(unsafe_code)]

mod auth;
mod auth_cookies;
mod auth_state;
mod pages;
mod routes;
mod state;

pub use routes::{router, router_with_auth};
