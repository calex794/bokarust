//! HTTP entry point. Route handlers are thin; decisions live in application/domain.
#![forbid(unsafe_code)]

mod auth;
mod auth_cookies;
mod author;
mod author_pages;
mod pages;
mod html_escape;
mod safe_markdown;
mod password_jobs;
mod reading_pages;
mod reading_private;
mod routes;

pub use routes::{router, router_with_auth, router_with_auth_and_reading};
