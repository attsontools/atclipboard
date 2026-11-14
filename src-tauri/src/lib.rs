use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

use atclipboard_service::{
    ClipboardServiceEvent, ServiceEventReceiver, ServiceSupervisor, ServiceSupervisorError,
    StandaloneService,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewWindow, WindowEvent};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct EventBridge {
    claimed_generation: AtomicU64,
    active_generation: AtomicU64,
}

impl EventBridge {
    fn claim(&self, generation: u64) -> Option<u64> {
        let mut claimed = self.claimed_generation.load(Ordering::Acquire);
        loop {
            if claimed >= generation {
                return None;
            }
            match self.claimed_generation.compare_exchange_weak(
                claimed,
                generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => claimed = current,
            }
        }
        self.active_generation
            .fetch_max(generation, Ordering::AcqRel);
        Some(generation)
    }

    fn release(&self, generation: u64) {
        let _ = self.active_generation.compare_exchange(
            generation,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    #[cfg(test)]
    fn active(&self) -> u64 {
        self.active_generation.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowActivation {
    id: String,
}

#[derive(Default)]
struct WindowActivations {
    preview: Mutex<Option<WindowActivation>>,
}

#[tauri::command]
async fn connect_clipboard_service(
    app: AppHandle,
    service: State<'_, StandaloneService>,
    bridge: State<'_, EventBridge>,
) -> Result<(), ServiceSupervisorError> {
    let connection_generation = service.connect().await?;
    if let Some(generation) = bridge.claim(connection_generation) {
        let events = match service.subscribe(connection_generation).await {
            Ok(events) => events,
            Err(error) => {
                bridge.release(generation);
                return Err(error);
            }
        };
        tauri::async_runtime::spawn(forward_service_events(app, events, generation));
    }
    Ok(())
}

#[tauri::command]
async fn clipboard_service_request(
    service: State<'_, StandaloneService>,
    method: String,
    params: Value,
) -> Result<Value, ServiceSupervisorError> {
    service.request(method, params).await
}

#[tauri::command]
fn open_clipboard_window(
    app: AppHandle,
    activations: State<'_, WindowActivations>,
    window_id: String,
    payload: Value,
) -> Result<(), String> {
    let activation = decode_window_activation(&window_id, payload)?;
    if let Some(activation) = &activation {
        *activations
            .preview
            .lock()
            .map_err(|_| "clipboard window state is unavailable".to_string())? =
            Some(activation.clone());
    }
    show_window(&app, &window_id, activation.as_ref())
}

#[tauri::command]
fn close_clipboard_window(app: AppHandle, window_id: String) -> Result<(), String> {
    close_window(&app, &window_id)
}

#[tauri::command]
fn get_clipboard_window_activation(
    activations: State<'_, WindowActivations>,
    window_id: String,
) -> Result<Option<Value>, String> {
    match window_id.as_str() {
        "preview" => Ok(activations
            .preview
            .lock()
            .map_err(|_| "clipboard window state is unavailable".to_string())?
            .as_ref()
            .map(activation_value)),
        "history" => Ok(None),
        _ => Err("clipboard window is not available".to_string()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let initial_window = requested_window(std::env::args());
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if let Err(error) = show_requested_window(app, requested_window(args)) {
                eprintln!("cannot activate clipboard window: {error}");
            }
        }))
        .setup(move |app| {
            let data_directory = shared_data_directory(app)?;
            let endpoint_path = data_directory.join("service.endpoint.json");
            let executable = service_executable(app)?;
            let supervisor = ServiceSupervisor::new(
                endpoint_path,
                data_directory,
                executable,
                "atclipboard-standalone",
                env!("CARGO_PKG_VERSION"),
                STARTUP_TIMEOUT,
            );
            app.manage(StandaloneService::new(supervisor));
            app.manage(EventBridge::default());
            app.manage(WindowActivations::default());
            show_requested_window(app.handle(), initial_window)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(window.label(), "main" | "history" | "preview") {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = close_window(window.app_handle(), window.label());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            connect_clipboard_service,
            clipboard_service_request,
            open_clipboard_window,
            close_clipboard_window,
            get_clipboard_window_activation
        ])
        .run(tauri::generate_context!())
        .expect("failed to run clipboard history");
}

fn requested_window<I, S>(args: I) -> &'static str
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if args
        .into_iter()
        .any(|argument| argument.as_ref() == "--history")
    {
        "history"
    } else {
        "main"
    }
}

fn show_requested_window(app: &AppHandle, window_id: &str) -> Result<(), String> {
    if window_id == "history" {
        return show_window(app, "history", None);
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "clipboard main window is not available".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

async fn forward_service_events(app: AppHandle, mut events: ServiceEventReceiver, generation: u64) {
    while let Some(event) = events.recv().await {
        match event {
            ClipboardServiceEvent::HistoryChanged { revision } => {
                let _ = app.emit(
                    "clipboard://history-changed",
                    json!({ "revision": revision }),
                );
            }
            ClipboardServiceEvent::HostUiOpen { window_id, payload } => {
                match decode_window_activation(&window_id, payload) {
                    Ok(activation) => {
                        if let Err(error) = show_window(&app, &window_id, activation.as_ref()) {
                            eprintln!("cannot open clipboard window: {error}");
                        }
                    }
                    Err(error) => eprintln!("rejected clipboard window activation: {error}"),
                }
            }
        }
    }
    app.state::<EventBridge>().release(generation);
}

fn decode_window_activation(
    window_id: &str,
    payload: Value,
) -> Result<Option<WindowActivation>, String> {
    match window_id {
        "history" if payload.as_object().is_some_and(|value| value.is_empty()) => Ok(None),
        "history" => Err("history activation payload must be empty".to_string()),
        "preview" => {
            let object = payload
                .as_object()
                .ok_or_else(|| "preview activation payload is invalid".to_string())?;
            if object.len() != 1 {
                return Err("preview activation payload must contain only an item id".to_string());
            }
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "preview activation item id is invalid".to_string())?;
            Ok(Some(WindowActivation { id: id.to_string() }))
        }
        _ => Err("clipboard window is not available".to_string()),
    }
}

fn activation_value(activation: &WindowActivation) -> Value {
    json!({ "id": activation.id })
}

fn show_window(
    app: &AppHandle,
    window_id: &str,
    activation: Option<&WindowActivation>,
) -> Result<(), String> {
    let window = app
        .get_webview_window(window_id)
        .ok_or_else(|| "clipboard window is not available".to_string())?;
    match window_id {
        "history" => position_history_window(&window),
        "preview" => {
            window.center().map_err(|error| error.to_string())?;
        }
        _ => return Err("clipboard window is not available".to_string()),
    }
    if let Some(activation) = activation {
        app.emit_to(
            window_id,
            &format!("clipboard://{window_id}-activation"),
            activation_value(activation),
        )
        .map_err(|error| error.to_string())?;
    }
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    Ok(())
}

fn close_window(app: &AppHandle, window_id: &str) -> Result<(), String> {
    if !matches!(window_id, "main" | "history" | "preview") {
        return Err("clipboard window is not available".to_string());
    }
    let window = app
        .get_webview_window(window_id)
        .ok_or_else(|| "clipboard window is not available".to_string())?;
    window.hide().map_err(|error| error.to_string())?;
    if window_id == "preview" {
        if let Some(history) = app.get_webview_window("history") {
            if history.is_visible().map_err(|error| error.to_string())? {
                position_history_window(&history);
                history.set_focus().map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

fn position_history_window(window: &WebviewWindow) {
    let monitor = match window.current_monitor() {
        Ok(Some(monitor)) => Some(monitor),
        Ok(None) | Err(_) => window.primary_monitor().ok().flatten(),
    };
    let Some(monitor) = monitor else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let margin = (20.0 * monitor.scale_factor()).round() as i32;
    let x = monitor_position.x + (monitor_size.width as i32 - size.width as i32) / 2;
    let y = monitor_position.y + monitor_size.height as i32 - size.height as i32 - margin;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

fn shared_data_directory(app: &tauri::App) -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("ATCLIPBOARD_DATA_DIR") {
        return Ok(PathBuf::from(path));
    }
    let standalone_directory = app.path().app_data_dir()?;
    shared_data_directory_from(&standalone_directory).map_err(Into::into)
}

fn shared_data_directory_from(standalone_directory: &std::path::Path) -> Result<PathBuf, String> {
    let application_data_root = standalone_directory
        .parent()
        .ok_or_else(|| "standalone app data directory has no parent".to_string())?;
    Ok(application_data_root
        .join("com.attson.attool")
        .join("clipboard"))
}

fn service_executable(app: &tauri::App) -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("ATCLIPBOARD_SERVICE_EXECUTABLE") {
        return Ok(PathBuf::from(path));
    }
    let executable_name = if cfg!(windows) {
        "atclipboard-service.exe"
    } else {
        "atclipboard-service"
    };
    Ok(app.path().resource_dir()?.join(executable_name))
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc, thread};

    use serde_json::json;

    use super::{
        decode_window_activation, requested_window, shared_data_directory_from, EventBridge,
    };

    #[test]
    fn standalone_resolves_the_existing_attool_clipboard_directory() {
        assert_eq!(
            shared_data_directory_from(Path::new("/data/com.attson.clipboard")).unwrap(),
            Path::new("/data/com.attson.attool/clipboard")
        );
    }

    #[test]
    fn preview_activation_accepts_only_an_item_id() {
        let activation = decode_window_activation("preview", json!({ "id": "clip-7" }))
            .unwrap()
            .unwrap();
        assert_eq!(activation.id, "clip-7");
        assert!(decode_window_activation(
            "preview",
            json!({ "id": "clip-7", "contentText": "must not cross the window boundary" })
        )
        .is_err());
        assert!(decode_window_activation("preview", json!({ "id": "" })).is_err());
        assert!(decode_window_activation("history", json!({}))
            .unwrap()
            .is_none());
    }

    #[test]
    fn stale_event_bridge_release_cannot_clear_the_replacement_bridge() {
        let bridge = Arc::new(EventBridge::default());
        let old_generation = bridge.claim(1).unwrap();
        let (release_old_tx, release_old_rx) = std::sync::mpsc::channel();
        let (released_old_tx, released_old_rx) = std::sync::mpsc::channel();
        let stale_bridge = Arc::clone(&bridge);
        let stale = thread::spawn(move || {
            release_old_rx.recv().unwrap();
            stale_bridge.release(old_generation);
            released_old_tx.send(()).unwrap();
        });

        let replacement_generation = bridge.claim(2).unwrap();
        release_old_tx.send(()).unwrap();
        released_old_rx.recv().unwrap();

        assert_eq!(bridge.active(), replacement_generation);
        assert!(bridge.claim(2).is_none());
        bridge.release(replacement_generation);
        assert!(bridge.claim(3).is_some());
        stale.join().unwrap();
    }

    #[test]
    fn one_service_connection_can_claim_only_one_event_bridge() {
        let bridge = EventBridge::default();

        assert_eq!(bridge.claim(7), Some(7));
        assert_eq!(bridge.claim(7), None);
        assert_eq!(bridge.claim(6), None);
        assert_eq!(bridge.claim(8), Some(8));
    }

    #[test]
    fn history_command_routes_to_the_existing_history_window() {
        assert_eq!(requested_window(["atclipboard", "--history"]), "history");
        assert_eq!(requested_window(["atclipboard"]), "main");
        assert_eq!(requested_window(["atclipboard", "--unknown"]), "main");
    }
}
