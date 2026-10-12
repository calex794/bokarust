//! HTTP entry point. Route handlers are thin; decisions live in application/domain.
#![forbid(unsafe_code)]

mod auth;
mod author;
mod author_pages;
mod auth_cookies;
mod pages;
mod password_jobs;
mod routes;

pub use routes::{router, router_with_auth, router_with_auth_and_reading};
