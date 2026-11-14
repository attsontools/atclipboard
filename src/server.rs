use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use fs2::FileExt;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{tcp::OwnedWriteHalf, TcpListener, TcpStream},
    sync::{Mutex, Notify},
};
use uuid::Uuid;

use crate::{
    negotiate, write_endpoint, ClipboardService, Endpoint, ErrorEnvelope, ServiceError,
    ServiceHandshake, ServiceReply, ServiceRequest,
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
        fs::create_dir_all(&data_directory).map_err(|_| lease_error())?;
        let token = Uuid::new_v4().to_string();
        let lease = ServiceLease::acquire(&data_directory, &token)?;
        let service = ClipboardService::open(&data_directory)?;
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
        loop {
            tokio::select! {
                _ = self.stop.notified() => break,
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted.map_err(|_| ServiceError::new(
                        "CLIPBOARD_SERVICE_ACCEPT_FAILED",
                        "cannot accept service connection",
                    ))?;
                    let service = self.service.clone();
                    let token = self.token.clone();
                    let data_directory = self.data_directory.to_string_lossy().into_owned();
                    tokio::spawn(async move {
                        let _ = handle_connection(stream, service, token, data_directory).await;
                    });
                }
            }
        }
        let _ = fs::remove_file(&self.endpoint_path);
        Ok(())
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

    loop {
        line.clear();
        match read_line(&mut reader, &mut line).await {
            Ok(false) => break,
            Ok(true) => {}
            Err(error) => return Err(error),
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
    }
    Ok(())
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
