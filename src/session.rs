//! A bounded wrapper around rmcp's in-memory session manager.

use std::sync::Arc;

use futures::Stream;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::WorkerTransport;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManager, LocalSessionManagerError, LocalSessionWorker,
};
use rmcp::transport::streamable_http_server::session::{
    EventStore, RestoreOutcome, ServerSseMessage, SessionId, SessionManager,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CappedSessionError {
    #[error("MCP session capacity reached ({limit})")]
    Capacity { limit: usize },
    #[error(transparent)]
    Local(#[from] LocalSessionManagerError),
}

#[derive(Debug)]
pub struct CappedSessionManager {
    inner: LocalSessionManager,
    max_sessions: usize,
    creation: tokio::sync::Mutex<()>,
}

impl CappedSessionManager {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            inner: LocalSessionManager::default(),
            max_sessions,
            creation: tokio::sync::Mutex::new(()),
        }
    }

    async fn ensure_capacity(&self) -> Result<(), CappedSessionError> {
        if self.inner.sessions.read().await.len() >= self.max_sessions {
            Err(CappedSessionError::Capacity {
                limit: self.max_sessions,
            })
        } else {
            Ok(())
        }
    }
}

impl SessionManager for CappedSessionManager {
    type Error = CappedSessionError;
    type Transport = WorkerTransport<LocalSessionWorker>;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        let _creation = self.creation.lock().await;
        self.ensure_capacity().await?;
        Ok(self.inner.create_session().await?)
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        Ok(self.inner.initialize_session(id, message).await?)
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        Ok(self.inner.has_session(id).await?)
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        Ok(self.inner.close_session(id).await?)
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.create_stream(id, message).await?)
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        Ok(self.inner.accept_message(id, message).await?)
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.create_standalone_stream(id).await?)
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.resume(id, last_event_id).await?)
    }

    async fn restore_session(
        &self,
        id: SessionId,
    ) -> Result<RestoreOutcome<Self::Transport>, Self::Error> {
        let _creation = self.creation.lock().await;
        if self.inner.has_session(&id).await? {
            return Ok(RestoreOutcome::AlreadyPresent);
        }
        self.ensure_capacity().await?;
        Ok(self.inner.restore_session(id).await?)
    }

    fn event_store(&self) -> Option<Arc<dyn EventStore>> {
        self.inner.event_store()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_new_sessions_at_capacity_and_recovers_after_close() {
        let manager = CappedSessionManager::new(1);
        let (first, _transport) = manager.create_session().await.expect("first session");
        assert!(matches!(
            manager.create_session().await,
            Err(CappedSessionError::Capacity { limit: 1 })
        ));
        manager.close_session(&first).await.expect("close");
        manager.create_session().await.expect("replacement session");
    }
}
