//! Shared request state; no route handler is referenced from here.
use crate::auth_state::AuthState;
use bokarust_application::catalog::PublicBookRepository;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) catalog: Arc<dyn PublicBookRepository>,
    pub(crate) auth: Option<AuthState>,
}
