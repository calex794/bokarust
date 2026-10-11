//! HTTP entry point. Route handlers are thin; decisions live in application/domain.
#![forbid(unsafe_code)]

mod pages;
mod routes;

pub use routes::router;
