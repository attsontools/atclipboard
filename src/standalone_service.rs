use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use serde_json::Value;
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::{ClipboardServiceEvent, ServiceClient, ServiceSupervisor, ServiceSupervisorError};

pub struct StandaloneService {
    supervisor: Mutex<ServiceSupervisor>,
    next_generation: AtomicU64,
    client: RwLock<Option<ServiceConnection>>,
}

struct ServiceConnection {
    generation: u64,
    client: Arc<ServiceClient>,
}

pub struct ServiceEventReceiver {
    receiver: broadcast::Receiver<ClipboardServiceEvent>,
}

impl ServiceEventReceiver {
    pub async fn recv(&mut self) -> Option<ClipboardServiceEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

impl StandaloneService {
    pub fn new(supervisor: ServiceSupervisor) -> Self {
        Self {
            supervisor: Mutex::new(supervisor),
            next_generation: AtomicU64::new(0),
            client: RwLock::new(None),
        }
    }

    pub async fn connect(&self) -> Result<u64, ServiceSupervisorError> {
        if let Some(connection) = self.client.read().await.as_ref() {
            if !connection.client.is_disconnected() {
                return Ok(connection.generation);
            }
        }
        let mut supervisor = self.supervisor.lock().await;
        if let Some(connection) = self.client.read().await.as_ref() {
            if !connection.client.is_disconnected() {
                return Ok(connection.generation);
            }
        }
        let client = Arc::new(supervisor.connect().await?);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        *self.client.write().await = Some(ServiceConnection { generation, client });
        Ok(generation)
    }

    pub async fn subscribe(
        &self,
        generation: u64,
    ) -> Result<ServiceEventReceiver, ServiceSupervisorError> {
        let client = self
            .client
            .read()
            .await
            .as_ref()
            .filter(|connection| connection.generation == generation)
            .map(|connection| Arc::clone(&connection.client))
            .ok_or_else(|| {
                ServiceSupervisorError::new(
                    "CLIPBOARD_SERVICE_CONNECTION_CHANGED",
                    "clipboard service connection changed before event subscription",
                )
            })?;
        Ok(ServiceEventReceiver {
            receiver: client.subscribe(),
        })
    }

    pub async fn request(
        &self,
        method: impl Into<String>,
        params: Value,
    ) -> Result<Value, ServiceSupervisorError> {
        let client = self
            .client
            .read()
            .await
            .as_ref()
            .map(|connection| Arc::clone(&connection.client))
            .ok_or_else(|| {
                ServiceSupervisorError::new(
                    "CLIPBOARD_SERVICE_NOT_CONNECTED",
                    "clipboard service has not been connected",
                )
            })?;
        client.request(method, params).await.map_err(Into::into)
    }
}
