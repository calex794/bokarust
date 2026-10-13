//! HTTP entry point. Route handlers are thin; decisions live in application/domain.
#![forbid(unsafe_code)]

mod auth;
mod auth_cookies;
mod checkout_pages;
mod checkout_private;
mod author;
mod author_pages;
mod html_escape;
mod pages;
mod password_jobs;
mod purchased_pages;
mod purchased_private;
mod reading_pages;
mod reading_private;
mod routes;
mod safe_markdown;

pub use routes::{router, router_with_auth, router_with_auth_and_reading};
