use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use image::{DynamicImage, RgbaImage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ClipboardBackend, ClipboardImage, ServiceError, SystemClipboardBackend};

const SNAPSHOT_MANIFEST_ENV: &str = "ATCLIPBOARD_SNAPSHOT_MANIFEST";
const SNAPSHOT_IMAGE_ENV: &str = "ATCLIPBOARD_SNAPSHOT_IMAGE";
const DEFAULT_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug)]
pub(crate) struct ClipboardCapture {
    pub(crate) files: Option<Vec<String>>,
    pub(crate) text: Option<String>,
    pub(crate) image: Option<ClipboardImage>,
}

pub(crate) trait ClipboardCaptureSource: Send + Sync {
    fn capture(&self) -> Result<ClipboardCapture, ServiceError>;
}

pub(crate) struct BackendClipboardCaptureSource {
    backend: Arc<dyn ClipboardBackend>,
}

impl BackendClipboardCaptureSource {
    pub(crate) fn new(backend: Arc<dyn ClipboardBackend>) -> Self {
        Self { backend }
    }
}

impl ClipboardCaptureSource for BackendClipboardCaptureSource {
    fn capture(&self) -> Result<ClipboardCapture, ServiceError> {
        capture_backend(self.backend.as_ref())
    }
}

pub(crate) struct ProcessClipboardCaptureSource {
    data_directory: PathBuf,
    timeout: Duration,
    launcher: Arc<dyn HelperLauncher>,
}

impl ProcessClipboardCaptureSource {
    pub(crate) fn new(data_directory: PathBuf) -> Result<Self, ServiceError> {
        let executable = std::env::current_exe().map_err(|_| snapshot_read_failed())?;
        Ok(Self {
            data_directory,
            timeout: DEFAULT_SNAPSHOT_TIMEOUT,
            launcher: Arc::new(CommandHelperLauncher { executable }),
        })
    }

    #[cfg(test)]
    fn with_launcher(
        data_directory: PathBuf,
        timeout: Duration,
        launcher: Arc<dyn HelperLauncher>,
    ) -> Self {
        Self {
            data_directory,
            timeout,
            launcher,
        }
    }
}

impl ClipboardCaptureSource for ProcessClipboardCaptureSource {
    fn capture(&self) -> Result<ClipboardCapture, ServiceError> {
        let capture_directory = self.data_directory.join("capture-snapshots");
        fs::create_dir_all(&capture_directory).map_err(|_| snapshot_read_failed())?;
        restrict_directory(&capture_directory)?;
        let id = Uuid::new_v4();
        let launch = HelperLaunch {
            manifest_path: capture_directory.join(format!("{id}.json")),
            image_path: capture_directory.join(format!("{id}.png")),
        };
        let cleanup = SnapshotCleanup::new(&launch);
        let mut process = self
            .launcher
            .launch(&launch)
            .map_err(|_| snapshot_read_failed())?;
        wait_for_helper(process.as_mut(), self.timeout)?;
        let manifest = fs::read(&launch.manifest_path)
            .map_err(|_| snapshot_output_invalid())
            .and_then(|bytes| {
                serde_json::from_slice::<SnapshotManifest>(&bytes)
                    .map_err(|_| snapshot_output_invalid())
            })?;
        let image = manifest
            .image
            .map(|expected| read_snapshot_image(&launch.image_path, expected))
            .transpose()?;
        drop(cleanup);
        Ok(ClipboardCapture {
            files: manifest.files,
            text: manifest.text,
            image,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotManifest {
    files: Option<Vec<String>>,
    text: Option<String>,
    image: Option<SnapshotImage>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotImage {
    width: u32,
    height: u32,
}

struct HelperLaunch {
    manifest_path: PathBuf,
    image_path: PathBuf,
}

trait HelperLauncher: Send + Sync {
    fn launch(&self, launch: &HelperLaunch) -> io::Result<Box<dyn HelperProcess>>;
}

struct CommandHelperLauncher {
    executable: PathBuf,
}

impl HelperLauncher for CommandHelperLauncher {
    fn launch(&self, launch: &HelperLaunch) -> io::Result<Box<dyn HelperProcess>> {
        Command::new(&self.executable)
            .env(SNAPSHOT_MANIFEST_ENV, &launch.manifest_path)
            .env(SNAPSHOT_IMAGE_ENV, &launch.image_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|child| Box::new(child) as Box<dyn HelperProcess>)
    }
}

trait HelperProcess: Send {
    fn try_exit(&mut self) -> io::Result<Option<i32>>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<()>;
}

impl HelperProcess for Child {
    fn try_exit(&mut self) -> io::Result<Option<i32>> {
        Child::try_wait(self).map(|status| status.map(|status| status.code().unwrap_or(-1)))
    }

    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }

    fn wait(&mut self) -> io::Result<()> {
        Child::wait(self).map(|_| ())
    }
}

fn wait_for_helper(process: &mut dyn HelperProcess, timeout: Duration) -> Result<(), ServiceError> {
    let deadline = Instant::now() + timeout;
    loop {
        match process.try_exit() {
            Err(_) => {
                terminate_helper(process);
                return Err(snapshot_read_failed());
            }
            Ok(Some(0)) => return Ok(()),
            Ok(Some(_)) => return Err(snapshot_read_failed()),
            Ok(None) if Instant::now() >= deadline => {
                terminate_helper(process);
                return Err(snapshot_read_timeout());
            }
            Ok(None) => thread::sleep(
                PROCESS_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
            ),
        }
    }
}

fn terminate_helper(process: &mut dyn HelperProcess) {
    if process.kill().is_ok() {
        let _ = process.wait();
    }
}

struct SnapshotCleanup {
    manifest_path: PathBuf,
    image_path: PathBuf,
}

impl SnapshotCleanup {
    fn new(launch: &HelperLaunch) -> Self {
        Self {
            manifest_path: launch.manifest_path.clone(),
            image_path: launch.image_path.clone(),
        }
    }
}

impl Drop for SnapshotCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.manifest_path);
        let _ = fs::remove_file(&self.image_path);
    }
}

pub fn run_snapshot_helper_if_requested() -> Option<Result<(), ServiceError>> {
    let manifest_path = std::env::var_os(SNAPSHOT_MANIFEST_ENV).map(PathBuf::from)?;
    let Some(image_path) = std::env::var_os(SNAPSHOT_IMAGE_ENV).map(PathBuf::from) else {
        return Some(Err(snapshot_output_invalid()));
    };
    Some(write_snapshot(
        &manifest_path,
        &image_path,
        &SystemClipboardBackend::new(),
    ))
}

fn write_snapshot(
    manifest_path: &Path,
    image_path: &Path,
    backend: &dyn ClipboardBackend,
) -> Result<(), ServiceError> {
    let capture = capture_backend(backend)?;
    let image = capture
        .image
        .map(|image| write_snapshot_image(image_path, image))
        .transpose()?;
    let manifest = SnapshotManifest {
        files: capture.files,
        text: capture.text,
        image,
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|_| snapshot_output_invalid())?;
    fs::write(manifest_path, bytes).map_err(|_| snapshot_output_invalid())
}

fn capture_backend(backend: &dyn ClipboardBackend) -> Result<ClipboardCapture, ServiceError> {
    let files = backend
        .read_files()
        .map_err(|_| snapshot_read_failed())?
        .filter(|paths| !paths.is_empty());
    let text = if files.is_some() {
        None
    } else {
        backend.read_text().map_err(|_| snapshot_read_failed())?
    };
    let image = backend.read_image().map_err(|_| snapshot_read_failed())?;
    Ok(ClipboardCapture { files, text, image })
}

fn write_snapshot_image(path: &Path, image: ClipboardImage) -> Result<SnapshotImage, ServiceError> {
    let rgba = RgbaImage::from_raw(image.width, image.height, image.rgba)
        .ok_or_else(snapshot_output_invalid)?;
    DynamicImage::ImageRgba8(rgba)
        .save(path)
        .map_err(|_| snapshot_output_invalid())?;
    Ok(SnapshotImage {
        width: image.width,
        height: image.height,
    })
}

fn read_snapshot_image(
    path: &Path,
    expected: SnapshotImage,
) -> Result<ClipboardImage, ServiceError> {
    let image = image::open(path)
        .map_err(|_| snapshot_output_invalid())?
        .to_rgba8();
    if image.width() != expected.width || image.height() != expected.height {
        return Err(snapshot_output_invalid());
    }
    Ok(ClipboardImage {
        rgba: image.into_raw(),
        width: expected.width,
        height: expected.height,
    })
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), ServiceError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| snapshot_read_failed())
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), ServiceError> {
    Ok(())
}

fn snapshot_read_failed() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_CLIPBOARD_READ_FAILED",
        "cannot read the system clipboard",
    )
}

fn snapshot_read_timeout() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_CLIPBOARD_READ_TIMEOUT",
        "system clipboard read timed out",
    )
}

fn snapshot_output_invalid() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_CLIPBOARD_SNAPSHOT_INVALID",
        "system clipboard snapshot is invalid",
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        fs, io,
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::{
        wait_for_helper, ClipboardCaptureSource, HelperLaunch, HelperLauncher, HelperProcess,
        ProcessClipboardCaptureSource, SnapshotManifest,
    };

    enum Action {
        HangWithPartialOutput,
        CompleteWithText,
    }

    struct ScriptedLauncher {
        actions: Mutex<VecDeque<Action>>,
        killed: Arc<Mutex<usize>>,
        waited: Arc<Mutex<usize>>,
    }

    impl HelperLauncher for ScriptedLauncher {
        fn launch(&self, launch: &HelperLaunch) -> io::Result<Box<dyn HelperProcess>> {
            let action = self.actions.lock().unwrap().pop_front().unwrap();
            match action {
                Action::HangWithPartialOutput => {
                    fs::write(&launch.manifest_path, b"partial").unwrap();
                    fs::write(&launch.image_path, b"partial image").unwrap();
                    Ok(Box::new(FakeProcess {
                        exit_code: None,
                        kill_fails: false,
                        killed: Arc::clone(&self.killed),
                        waited: Arc::clone(&self.waited),
                    }))
                }
                Action::CompleteWithText => {
                    let manifest = SnapshotManifest {
                        files: None,
                        text: Some("recovered clipboard".into()),
                        image: None,
                    };
                    fs::write(
                        &launch.manifest_path,
                        serde_json::to_vec(&manifest).unwrap(),
                    )
                    .unwrap();
                    Ok(Box::new(FakeProcess {
                        exit_code: Some(0),
                        kill_fails: false,
                        killed: Arc::clone(&self.killed),
                        waited: Arc::clone(&self.waited),
                    }))
                }
            }
        }
    }

    struct FakeProcess {
        exit_code: Option<i32>,
        kill_fails: bool,
        killed: Arc<Mutex<usize>>,
        waited: Arc<Mutex<usize>>,
    }

    impl HelperProcess for FakeProcess {
        fn try_exit(&mut self) -> io::Result<Option<i32>> {
            Ok(self.exit_code)
        }

        fn kill(&mut self) -> io::Result<()> {
            if self.kill_fails {
                return Err(io::Error::other("kill failed"));
            }
            *self.killed.lock().unwrap() += 1;
            self.exit_code = Some(9);
            Ok(())
        }

        fn wait(&mut self) -> io::Result<()> {
            *self.waited.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[test]
    fn a_timed_out_helper_is_killed_reaped_and_the_next_capture_can_succeed() {
        let temporary = tempfile::tempdir().unwrap();
        let killed = Arc::new(Mutex::new(0));
        let waited = Arc::new(Mutex::new(0));
        let launcher = Arc::new(ScriptedLauncher {
            actions: Mutex::new(VecDeque::from([
                Action::HangWithPartialOutput,
                Action::CompleteWithText,
            ])),
            killed: Arc::clone(&killed),
            waited: Arc::clone(&waited),
        });
        let source = ProcessClipboardCaptureSource::with_launcher(
            temporary.path().to_path_buf(),
            Duration::ZERO,
            launcher,
        );

        let error = source.capture().unwrap_err();

        assert_eq!(error.code, "CLIPBOARD_SERVICE_CLIPBOARD_READ_TIMEOUT");
        assert_eq!(*killed.lock().unwrap(), 1);
        assert_eq!(*waited.lock().unwrap(), 1);
        assert!(capture_directory_is_empty(temporary.path()));

        let recovered = source.capture().unwrap();
        assert_eq!(recovered.files, None);
        assert_eq!(recovered.text.as_deref(), Some("recovered clipboard"));
        assert!(recovered.image.is_none());
        assert!(capture_directory_is_empty(temporary.path()));
    }

    #[test]
    fn a_failed_kill_does_not_wait_forever_on_the_helper() {
        let killed = Arc::new(Mutex::new(0));
        let waited = Arc::new(Mutex::new(0));
        let mut process = FakeProcess {
            exit_code: None,
            kill_fails: true,
            killed: Arc::clone(&killed),
            waited: Arc::clone(&waited),
        };

        let error = wait_for_helper(&mut process, Duration::ZERO).unwrap_err();

        assert_eq!(error.code, "CLIPBOARD_SERVICE_CLIPBOARD_READ_TIMEOUT");
        assert_eq!(*killed.lock().unwrap(), 0);
        assert_eq!(*waited.lock().unwrap(), 0);
    }

    fn capture_directory_is_empty(root: &Path) -> bool {
        let capture = root.join("capture-snapshots");
        !capture.exists() || fs::read_dir(capture).unwrap().next().is_none()
    }
}
