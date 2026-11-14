use std::{borrow::Cow, sync::Mutex};

use arboard::{Clipboard, Error as ClipboardError, ImageData};

use crate::{
    shortcut::ShortcutController, ClipboardBackend, ClipboardImage, ClipboardShortcutStatus,
};

pub struct SystemClipboardBackend {
    clipboard: Mutex<Option<Clipboard>>,
    shortcut: ShortcutController,
}

impl SystemClipboardBackend {
    pub fn new() -> Self {
        Self {
            clipboard: Mutex::new(None),
            shortcut: ShortcutController::unavailable(
                "global shortcut runtime is not initialized",
                system_shortcut_command(),
            ),
        }
    }

    pub(crate) fn with_shortcut(shortcut: ShortcutController) -> Self {
        Self {
            clipboard: Mutex::new(None),
            shortcut,
        }
    }

    fn with_clipboard<T>(
        &self,
        operation: impl FnOnce(&mut Clipboard) -> Result<T, ClipboardError>,
    ) -> Result<T, ClipboardError> {
        let mut clipboard = self.clipboard.lock().map_err(|_| ClipboardError::Unknown {
            description: "system clipboard state is unavailable".into(),
        })?;
        if clipboard.is_none() {
            *clipboard = Some(Clipboard::new()?);
        }
        operation(clipboard.as_mut().expect("clipboard initialized"))
    }
}

impl Default for SystemClipboardBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardBackend for SystemClipboardBackend {
    fn read_files(&self) -> Result<Option<Vec<String>>, String> {
        match self.with_clipboard(|clipboard| clipboard.get().file_list()) {
            Ok(paths) if paths.is_empty() => Ok(None),
            Ok(paths) => Ok(Some(
                paths
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
            )),
            Err(ClipboardError::ContentNotAvailable) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn read_text(&self) -> Result<Option<String>, String> {
        match self.with_clipboard(Clipboard::get_text) {
            Ok(value) if value.is_empty() => Ok(None),
            Ok(value) => Ok(Some(value)),
            Err(ClipboardError::ContentNotAvailable) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn read_image(&self) -> Result<Option<ClipboardImage>, String> {
        match self.with_clipboard(Clipboard::get_image) {
            Ok(image) => Ok(Some(ClipboardImage {
                rgba: image.bytes.into_owned(),
                width: u32::try_from(image.width)
                    .map_err(|_| "clipboard image width exceeds the supported range")?,
                height: u32::try_from(image.height)
                    .map_err(|_| "clipboard image height exceeds the supported range")?,
            })),
            Err(ClipboardError::ContentNotAvailable) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn write_text(&self, value: &str) -> Result<(), String> {
        self.with_clipboard(|clipboard| clipboard.set_text(value.to_string()))
            .map_err(|error| error.to_string())
    }

    fn write_image(&self, value: &image::DynamicImage) -> Result<(), String> {
        write_platform_image(self, value)
    }

    fn shortcut_status(&self) -> ClipboardShortcutStatus {
        self.shortcut.status()
    }

    fn replace_shortcut(&self, previous: &str, next: &str) -> Result<(), String> {
        self.shortcut.replace(previous, next)
    }

    fn initialize_shortcut(&self, shortcut: &str) {
        self.shortcut.initialize(shortcut);
    }

    fn set_open_history_handler(&self, handler: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.shortcut.set_open_history_handler(handler);
    }
}

fn write_arboard_image(
    backend: &SystemClipboardBackend,
    value: &image::DynamicImage,
) -> Result<(), String> {
    let rgba = value.to_rgba8();
    let (width, height) = rgba.dimensions();
    backend
        .with_clipboard(|clipboard| {
            clipboard.set_image(ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Owned(rgba.into_raw()),
            })
        })
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
fn write_platform_image(
    backend: &SystemClipboardBackend,
    value: &image::DynamicImage,
) -> Result<(), String> {
    write_arboard_image(backend, value)
}

#[cfg(target_os = "linux")]
fn write_platform_image(
    backend: &SystemClipboardBackend,
    value: &image::DynamicImage,
) -> Result<(), String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if std::env::var_os("DISPLAY").is_some() {
            return write_x11_image(value);
        }
        return write_wayland_image(value);
    }
    write_arboard_image(backend, value)
}

#[cfg(target_os = "linux")]
const X11_OWNER_ENV: &str = "ATCLIPBOARD_X11_CLIPBOARD_OWNER";

#[cfg(target_os = "linux")]
const X11_OWNER_START_ATTEMPTS: usize = 3;

#[cfg(target_os = "linux")]
const X11_OWNER_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(40);

#[cfg(target_os = "linux")]
static X11_OWNER_PROCESS: std::sync::OnceLock<std::sync::Mutex<Option<std::process::Child>>> =
    std::sync::OnceLock::new();

#[cfg(target_os = "linux")]
fn write_x11_image(value: &image::DynamicImage) -> Result<(), String> {
    use image::ImageFormat;
    use std::io::Cursor;

    let mut encoded = Cursor::new(Vec::new());
    value
        .write_to(&mut encoded, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    let process = X11_OWNER_PROCESS.get_or_init(|| std::sync::Mutex::new(None));
    let mut process = process
        .lock()
        .map_err(|_| "X11 clipboard owner state is unavailable".to_string())?;
    replace_owner_with_retry(
        &mut process,
        X11_OWNER_START_ATTEMPTS,
        || start_x11_owner(encoded.get_ref()),
        |owner| {
            let _ = owner.kill();
            let _ = owner.wait();
        },
        || std::thread::sleep(X11_OWNER_RETRY_DELAY),
    )
}

#[cfg(target_os = "linux")]
fn replace_owner_with_retry<T>(
    owner: &mut Option<T>,
    attempts: usize,
    mut start_candidate: impl FnMut() -> Result<T, String>,
    mut stop_owner: impl FnMut(&mut T),
    mut wait_before_retry: impl FnMut(),
) -> Result<(), String> {
    let attempts = attempts.max(1);
    let mut last_error = None;
    for attempt in 0..attempts {
        match start_candidate() {
            Ok(candidate) => {
                if let Some(mut previous) = owner.replace(candidate) {
                    stop_owner(&mut previous);
                }
                return Ok(());
            }
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < attempts {
                    wait_before_retry();
                }
            }
        }
    }
    Err(last_error.expect("at least one owner start attempt"))
}

#[cfg(target_os = "linux")]
fn start_x11_owner(encoded: &[u8]) -> Result<std::process::Child, String> {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };

    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut child = Command::new(executable)
        .env(X11_OWNER_ENV, "1")
        .env("GDK_BACKEND", "x11")
        .env("ATCLIPBOARD_PARENT_PID", std::process::id().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let ready = (|| {
        child
            .stdin
            .take()
            .ok_or_else(|| "X11 clipboard owner stdin is unavailable".to_string())?
            .write_all(encoded)
            .map_err(|error| error.to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "X11 clipboard owner stdout is unavailable".to_string())?;
        let mut response = String::new();
        BufReader::new(stdout)
            .read_line(&mut response)
            .map_err(|error| error.to_string())?;
        if response.trim() != "READY" {
            return Err(response
                .strip_prefix("ERROR ")
                .unwrap_or("X11 clipboard owner failed to start")
                .trim()
                .to_string());
        }
        Ok(())
    })();
    if let Err(error) = ready {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}

#[cfg(target_os = "linux")]
pub fn run_x11_clipboard_owner_if_requested() -> bool {
    if std::env::var_os(X11_OWNER_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return false;
    }
    if let Err(error) = run_x11_clipboard_owner() {
        println!("ERROR {}", error.replace(['\r', '\n'], " "));
    }
    true
}

#[cfg(not(target_os = "linux"))]
pub fn run_x11_clipboard_owner_if_requested() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn run_x11_clipboard_owner() -> Result<(), String> {
    use std::io::{Read, Write};

    gtk::init().map_err(|error| error.to_string())?;
    let mut encoded = Vec::new();
    std::io::stdin()
        .read_to_end(&mut encoded)
        .map_err(|error| error.to_string())?;
    let image = image::load_from_memory(&encoded)
        .map_err(|error| error.to_string())?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let width = i32::try_from(width).map_err(|_| "image width exceeds X11 limit".to_string())?;
    let height = i32::try_from(height).map_err(|_| "image height exceeds X11 limit".to_string())?;
    let row_stride = width
        .checked_mul(4)
        .ok_or_else(|| "image row stride exceeds X11 limit".to_string())?;
    let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_mut_slice(
        image.into_raw(),
        gtk::gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        width,
        height,
        row_stride,
    );
    let display =
        gtk::gdk::Display::default().ok_or_else(|| "X11 display is unavailable".to_string())?;
    let clipboard = gtk::Clipboard::default(&display)
        .ok_or_else(|| "X11 clipboard is unavailable".to_string())?;
    clipboard.set_image(&pixbuf);
    display.flush();
    let parent_pid = std::env::var("ATCLIPBOARD_PARENT_PID").unwrap_or_default();
    gtk::glib::timeout_add_seconds_local(1, move || {
        if parent_pid.is_empty() || std::path::Path::new("/proc").join(&parent_pid).exists() {
            gtk::glib::ControlFlow::Continue
        } else {
            gtk::main_quit();
            gtk::glib::ControlFlow::Break
        }
    });
    println!("READY");
    std::io::stdout()
        .flush()
        .map_err(|error| error.to_string())?;
    gtk::main();
    Ok(())
}

#[cfg(target_os = "linux")]
fn write_wayland_image(value: &image::DynamicImage) -> Result<(), String> {
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;
    use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};

    let rgba = value.to_rgba8();
    let rgb = RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y);
        let alpha = u16::from(pixel[3]);
        Rgb(std::array::from_fn(|channel| {
            ((u16::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
        }))
    });
    let png_image = DynamicImage::ImageRgba8(rgba);
    let fallback_image = DynamicImage::ImageRgb8(rgb);
    let encode = |source: &DynamicImage, format| {
        let mut output = Cursor::new(Vec::new());
        source
            .write_to(&mut output, format)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(output.into_inner())
    };
    let png = encode(&png_image, ImageFormat::Png)?;
    let bmp = encode(&fallback_image, ImageFormat::Bmp)?;
    let jpeg = encode(&fallback_image, ImageFormat::Jpeg)?;
    Options::new()
        .copy_multi(vec![
            MimeSource {
                source: Source::Bytes(png.into_boxed_slice()),
                mime_type: MimeType::Specific("image/png".into()),
            },
            MimeSource {
                source: Source::Bytes(bmp.into_boxed_slice()),
                mime_type: MimeType::Specific("image/bmp".into()),
            },
            MimeSource {
                source: Source::Bytes(jpeg.clone().into_boxed_slice()),
                mime_type: MimeType::Specific("image/jpeg".into()),
            },
            MimeSource {
                source: Source::Bytes(jpeg.into_boxed_slice()),
                mime_type: MimeType::Specific("image/jpg".into()),
            },
        ])
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
pub(crate) fn system_shortcut_command() -> Option<String> {
    Some("atclipboard --history".into())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn system_shortcut_command() -> Option<String> {
    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::{
        cell::Cell,
        sync::{mpsc, Arc, Barrier, Mutex},
        thread,
    };

    use super::replace_owner_with_retry;

    #[test]
    fn x11_owner_start_retries_without_releasing_the_previous_owner() {
        let mut owner = Some("previous");
        let attempts = Cell::new(0);
        let waits = Cell::new(0);
        let mut stopped = Vec::new();

        replace_owner_with_retry(
            &mut owner,
            3,
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                (attempt == 3)
                    .then_some("candidate")
                    .ok_or_else(|| format!("failure {attempt}"))
            },
            |value| stopped.push(*value),
            || waits.set(waits.get() + 1),
        )
        .unwrap();

        assert_eq!(attempts.get(), 3);
        assert_eq!(waits.get(), 2);
        assert_eq!(owner, Some("candidate"));
        assert_eq!(stopped, ["previous"]);
    }

    #[test]
    fn concurrent_x11_owner_replacements_are_serialized() {
        let owner = Arc::new(Mutex::new(Some("initial")));
        let stopped = Arc::new(Mutex::new(Vec::new()));
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();

        let first_owner = Arc::clone(&owner);
        let first_stopped = Arc::clone(&stopped);
        let first = thread::spawn(move || {
            let mut owner = first_owner.lock().unwrap();
            replace_owner_with_retry(
                &mut owner,
                1,
                || {
                    first_started_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    Ok("first")
                },
                |value| first_stopped.lock().unwrap().push(*value),
                || {},
            )
        });
        first_started_rx.recv().unwrap();

        let second_ready = Arc::new(Barrier::new(2));
        let second_owner = Arc::clone(&owner);
        let second_stopped = Arc::clone(&stopped);
        let second_ready_thread = Arc::clone(&second_ready);
        let second = thread::spawn(move || {
            second_ready_thread.wait();
            let mut owner = second_owner.lock().unwrap();
            replace_owner_with_retry(
                &mut owner,
                1,
                || Ok("second"),
                |value| second_stopped.lock().unwrap().push(*value),
                || {},
            )
        });
        second_ready.wait();
        release_first_tx.send(()).unwrap();

        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert_eq!(*owner.lock().unwrap(), Some("second"));
        assert_eq!(*stopped.lock().unwrap(), ["initial", "first"]);
    }
}
