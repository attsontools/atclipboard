mod client;
mod endpoint;
mod protocol;
mod server;
mod service;
mod service_cli;
mod standalone_service;
mod storage;
mod supervisor;

pub use client::{ServiceClient, ServiceClientError};
pub use endpoint::{read_endpoint, write_endpoint, Endpoint};
pub use protocol::{
    negotiate, ErrorEnvelope, HandshakeRequest, ServiceCapabilities, ServiceHandshake,
    ServiceHandshakeResponse, ServiceReply, ServiceRequest, APP_ID, DATA_API, HOST_API,
    SERVICE_API,
};
pub use server::ServiceServer;
pub use service::ClipboardService;
pub use service_cli::{parse_service_args, ServiceRunOptions};
pub use standalone_service::StandaloneService;
pub use storage::{ClipboardHistoryItem, ClipboardItemKind, ClipboardStore, TakeoverOutcome};
pub use supervisor::{
    CommandServiceLauncher, ServiceLaunchSpec, ServiceLauncher, ServiceProcess, ServiceSupervisor,
    ServiceSupervisorError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl ServiceError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
        }
    }
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ServiceError {}
