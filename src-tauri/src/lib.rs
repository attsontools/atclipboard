use std::{path::PathBuf, time::Duration};

use atclipboard_service::{ServiceSupervisor, ServiceSupervisorError, StandaloneService};
use serde_json::Value;
use tauri::{Manager, State};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[tauri::command]
async fn connect_clipboard_service(
    service: State<'_, StandaloneService>,
) -> Result<(), ServiceSupervisorError> {
    service.connect().await
}

#[tauri::command]
async fn clipboard_service_request(
    service: State<'_, StandaloneService>,
    method: String,
    params: Value,
) -> Result<Value, ServiceSupervisorError> {
    service.request(method, params).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_clipboard_service,
            clipboard_service_request
        ])
        .run(tauri::generate_context!())
        .expect("failed to run clipboard history");
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
    use std::path::Path;

    use super::shared_data_directory_from;

    #[test]
    fn standalone_resolves_the_existing_attool_clipboard_directory() {
        assert_eq!(
            shared_data_directory_from(Path::new("/data/com.attson.clipboard")).unwrap(),
            Path::new("/data/com.attson.attool/clipboard")
        );
    }
}
