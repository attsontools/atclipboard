use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

use fs2::FileExt;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{tcp::OwnedWriteHalf, TcpListener, TcpStream},
    sync::{broadcast, Mutex, Notify},
    task::JoinSet,
};
use uuid::Uuid;

use crate::{
    negotiate, write_endpoint, ClipboardBackend, ClipboardRuntime, ClipboardService, Endpoint,
    ErrorEnvelope, ServiceError, ServiceHandshake, ServiceReply, ServiceRequest,
    SystemClipboardBackend,
};

const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

pub struct ServiceServer {
    listener: TcpListener,
    endpoint_path: PathBuf,
    data_directory: PathBuf,
    token: String,
    service: ClipboardService,
    stop: Arc<Notify>,
    _lease: ServiceLease,
}

impl ServiceServer {
    pub async fn bind(
        endpoint_path: PathBuf,
        data_directory: PathBuf,
        service_version: impl Into<String>,
    ) -> Result<Self, ServiceError> {
        let service = ClipboardService::open_with_backend(
            &data_directory,
            Arc::new(SystemClipboardBackend::new()),
        )?;
        Self::bind_service(endpoint_path, data_directory, service_version, service).await
    }

    pub async fn bind_with_backend(
        endpoint_path: PathBuf,
        data_directory: PathBuf,
        service_version: impl Into<String>,
        backend: Arc<dyn ClipboardBackend>,
    ) -> Result<Self, ServiceError> {
        let service = ClipboardService::open_with_backend(&data_directory, backend)?;
        Self::bind_service(endpoint_path, data_directory, service_version, service).await
    }

    async fn bind_service(
        endpoint_path: PathBuf,
        data_directory: PathBuf,
        service_version: impl Into<String>,
        service: ClipboardService,
    ) -> Result<Self, ServiceError> {
        fs::create_dir_all(&data_directory).map_err(|_| lease_error())?;
        let token = Uuid::new_v4().to_string();
        let lease = ServiceLease::acquire(&data_directory, &token)?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| start_error())?;
        let port = listener.local_addr().map_err(|_| start_error())?.port();
        write_endpoint(
            &endpoint_path,
            &Endpoint {
                port,
                token: token.clone(),
                service_version: service_version.into(),
            },
        )?;
        let history_service = service.clone();
        service
            .backend
            .set_open_history_handler(Arc::new(move || history_service.open_history()));
        if let Ok(settings) = service.store.load_settings() {
            service.backend.initialize_shortcut(&settings.shortcut);
        }
        Ok(Self {
            listener,
            endpoint_path,
            data_directory,
            token,
            service,
            stop: Arc::new(Notify::new()),
            _lease: lease,
        })
    }

    pub fn stop_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.stop)
    }

    pub async fn run(self) -> Result<(), ServiceError> {
        let _watcher = BackgroundWatcher::start(ClipboardRuntime::new(self.service.clone()));
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                _ = self.stop.notified() => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted.map_err(|_| ServiceError::new(
                        "CLIPBOARD_SERVICE_ACCEPT_FAILED",
                        "cannot accept service connection",
                    ))?;
                    let service = self.service.clone();
                    let token = self.token.clone();
                    let data_directory = self.data_directory.to_string_lossy().into_owned();
                    connections.spawn(async move {
                        let _ = handle_connection(stream, service, token, data_directory).await;
                    });
                }
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        let _ = fs::remove_file(&self.endpoint_path);
        Ok(())
    }
}

struct BackgroundWatcher {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl BackgroundWatcher {
    fn start(runtime: ClipboardRuntime) -> Self {
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut last_error = None;
            loop {
                match stopped.recv_timeout(Duration::from_millis(800)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => match runtime.tick() {
                        Ok(_) => last_error = None,
                        Err(error) => {
                            if last_error != Some(error.code) {
                                eprintln!("{}: {}", error.code, error.message);
                            }
                            last_error = Some(error.code);
                        }
                    },
                }
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for BackgroundWatcher {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    service: ClipboardService,
    token: String,
    data_directory: String,
) -> Result<(), ServiceError> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let mut line = Vec::new();
    read_line(&mut reader, &mut line).await?;
    let handshake = serde_json::from_slice::<ServiceHandshake>(&line).map_err(|_| {
        ServiceError::new(
            "CLIPBOARD_SERVICE_REQUEST_INVALID",
            "cannot decode service handshake",
        )
    })?;
    let request = match handshake {
        ServiceHandshake::Request(request) => request,
        ServiceHandshake::Response(_) => {
            return Err(ServiceError::new(
                "CLIPBOARD_SERVICE_REQUEST_INVALID",
                "expected handshake request",
            ));
        }
    };
    let response = match negotiate(&request, &token, &data_directory) {
        Ok(response) => ServiceReply::Handshake(response),
        Err(error) => {
            write_reply(
                &writer,
                &ServiceReply::Error {
                    request_id: request.request_id,
                    error: error.into(),
                },
            )
            .await?;
            return Ok(());
        }
    };
    write_reply(&writer, &response).await?;

    let mut events = service.subscribe();
    line.clear();
    loop {
        tokio::select! {
            event = receive_next_event(&mut events) => {
                let Some(event) = event else {
                    break;
                };
                write_reply(&writer, &ServiceReply::Event { event }).await?;
                continue;
            }
            read = read_line(&mut reader, &mut line) => match read? {
                false => break,
                true => {}
            },
        }
        let request = match serde_json::from_slice::<ServiceRequest>(&line) {
            Ok(request) => request,
            Err(_) => {
                write_reply(
                    &writer,
                    &ServiceReply::Error {
                        request_id: String::new(),
                        error: ErrorEnvelope {
                            code: "CLIPBOARD_SERVICE_REQUEST_INVALID".into(),
                            message: "cannot decode service request".into(),
                            retryable: false,
                        },
                    },
                )
                .await?;
                line.clear();
                continue;
            }
        };
        let ServiceRequest::Request {
            request_id,
            method,
            params,
        } = request;
        let reply = match service.request(&method, params).await {
            Ok(result) => ServiceReply::Response { request_id, result },
            Err(error) => ServiceReply::Error {
                request_id,
                error: error.into(),
            },
        };
        write_reply(&writer, &reply).await?;
        line.clear();
    }
    Ok(())
}

async fn receive_next_event(
    events: &mut broadcast::Receiver<crate::ClipboardServiceEvent>,
) -> Option<crate::ClipboardServiceEvent> {
    loop {
        match events.recv().await {
            Ok(event) => return Some(event),
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return None,
        }
    }
}

async fn read_line<R>(reader: &mut R, line: &mut Vec<u8>) -> Result<bool, ServiceError>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let available = reader.fill_buf().await.map_err(|_| disconnected())?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        let chunk_len = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|position| position + 1)
            .unwrap_or(available.len());
        let remaining = MAX_LINE_BYTES.saturating_add(1).saturating_sub(line.len());
        let consumed = chunk_len.min(remaining);
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if line.len() > MAX_LINE_BYTES {
            return Err(ServiceError::new(
                "CLIPBOARD_SERVICE_REQUEST_TOO_LARGE",
                "service message exceeds 8 MiB",
            ));
        }
        if consumed == chunk_len && line.last() == Some(&b'\n') {
            return Ok(true);
        }
    }
}

async fn write_reply(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    reply: &ServiceReply,
) -> Result<(), ServiceError> {
    let mut bytes = serde_json::to_vec(reply).map_err(|_| {
        ServiceError::new(
            "CLIPBOARD_SERVICE_RESPONSE_INVALID",
            "cannot encode service response",
        )
    })?;
    bytes.push(b'\n');
    writer
        .lock()
        .await
        .write_all(&bytes)
        .await
        .map_err(|_| disconnected())
}

struct ServiceLease {
    path: PathBuf,
    token: String,
    file: File,
}

impl ServiceLease {
    fn acquire(data_directory: &Path, token: &str) -> Result<Self, ServiceError> {
        let path = data_directory.join("service.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|_| lease_error())?;
        file.try_lock_exclusive().map_err(|_| {
            ServiceError::new(
                "CLIPBOARD_SERVICE_ALREADY_RUNNING",
                "clipboard service is already running",
            )
        })?;
        file.set_len(0).map_err(|_| lease_error())?;
        writeln!(file, "{token}\n{}", std::process::id()).map_err(|_| lease_error())?;
        file.sync_all().map_err(|_| lease_error())?;
        Ok(Self {
            path,
            token: token.into(),
            file,
        })
    }
}

impl Drop for ServiceLease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        if fs::read_to_string(&self.path)
            .ok()
            .and_then(|value| value.lines().next().map(ToOwned::to_owned))
            .as_deref()
            == Some(self.token.as_str())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn start_error() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_START_FAILED",
        "cannot start clipboard service",
    )
}

fn lease_error() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_LEASE_FAILED",
        "cannot acquire clipboard service lease",
    )
}

fn disconnected() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_DISCONNECTED",
        "clipboard service connection was closed",
    )
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast;

    use super::receive_next_event;
    use crate::ClipboardServiceEvent;

    #[tokio::test]
    async fn lagged_event_receivers_continue_with_the_latest_event() {
        let (events, mut receiver) = broadcast::channel(1);
        events
            .send(ClipboardServiceEvent::HistoryChanged { revision: 1 })
            .unwrap();
        events
            .send(ClipboardServiceEvent::HistoryChanged { revision: 2 })
            .unwrap();

        assert_eq!(
            receive_next_event(&mut receiver).await,
            Some(ClipboardServiceEvent::HistoryChanged { revision: 2 })
        );
        events
            .send(ClipboardServiceEvent::HistoryChanged { revision: 3 })
            .unwrap();
        assert_eq!(
            receive_next_event(&mut receiver).await,
            Some(ClipboardServiceEvent::HistoryChanged { revision: 3 })
        );
    }
}
