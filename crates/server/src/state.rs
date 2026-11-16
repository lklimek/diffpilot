//! Shared application state handed to every handler.

use std::sync::Arc;

use crate::config::Config;

/// State shared by all handlers; cheap to clone.
#[derive(Clone, Debug)]
pub struct AppState {
    /// Server configuration.
    pub config: Arc<Config>,
}

impl AppState {
    /// Builds the state from a loaded configuration.
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}
