mod client;
mod endpoint;
mod platform;
mod protocol;
mod runtime;
mod server;
mod service;
mod service_cli;
mod shortcut;
mod standalone_service;
mod storage;
mod supervisor;

pub use client::{ServiceClient, ServiceClientError};
pub use endpoint::{read_endpoint, write_endpoint, Endpoint};
pub use platform::{run_x11_clipboard_owner_if_requested, SystemClipboardBackend};
pub use protocol::{
    negotiate, ClipboardServiceEvent, ErrorEnvelope, HandshakeRequest, ServiceCapabilities,
    ServiceHandshake, ServiceHandshakeResponse, ServiceReply, ServiceRequest, APP_ID, DATA_API,
    HOST_API, SERVICE_API,
};
pub use runtime::{ClipboardRuntime, ClipboardRuntimeTick};
pub use server::ServiceServer;
pub use service::{ClipboardBackend, ClipboardImage, ClipboardService, ClipboardShortcutStatus};
pub use service_cli::{parse_service_args, ServiceRunOptions};
pub use shortcut::run_service_process;
pub use standalone_service::{ServiceEventReceiver, StandaloneService};
pub use storage::{
    ClipboardHistoryItem, ClipboardItemKind, ClipboardSettings, ClipboardStore, TakeoverOutcome,
};
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
