use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::watch;

use crate::session::Session;

pub mod mcp;

#[derive(Debug, Clone)]
pub struct ShutdownToken {
    sender: watch::Sender<bool>,
    receiver: watch::Receiver<bool>,
}

impl ShutdownToken {
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel(false);
        Self { sender, receiver }
    }

    pub fn cancel(&self) {
        let _ = self.sender.send(true);
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.receiver.clone();
        if *receiver.borrow() {
            return;
        }
        let _ = receiver.changed().await;
    }
}

impl Default for ShutdownToken {
    fn default() -> Self {
        Self::new()
    }
}

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
