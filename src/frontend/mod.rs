use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::session::Session;

pub mod mcp;

#[derive(Debug, Clone)]
pub struct ShutdownToken;

#[derive(Debug, Error)]
pub enum FrontendError {
    #[error("mcp frontend failed: {0}")]
    Mcp(String),
}

#[async_trait]
pub trait Frontend: Send + Sync {
    async fn serve(
        self,
        session: Arc<Session>,
        shutdown: ShutdownToken,
    ) -> Result<(), FrontendError>;
}
