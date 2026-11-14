use std::{
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex},
};

use image::{DynamicImage, ImageFormat, RgbaImage};

use crate::{ClipboardImage, ClipboardService, ServiceError};

#[derive(Clone)]
pub struct ClipboardRuntime {
    service: ClipboardService,
    last_textual_state: Arc<Mutex<Option<TextualClipboardState>>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipboardRuntimeTick {
    pub inserted: usize,
    pub deleted_by_retention: usize,
}

impl ClipboardRuntime {
    pub fn new(service: ClipboardService) -> Self {
        Self {
            service,
            last_textual_state: Arc::new(Mutex::new(None)),
        }
    }

    pub fn tick(&self) -> Result<ClipboardRuntimeTick, ServiceError> {
        let settings = self.service.store.load_settings()?;
        if !settings.capture_enabled {
            return Ok(ClipboardRuntimeTick::default());
        }
        let files = self
            .service
            .backend
            .read_files()
            .map_err(clipboard_read_failed)?
            .filter(|paths| !paths.is_empty());
        let textual_state = if let Some(paths) = files {
            Some(TextualClipboardState::Files(paths))
        } else if let Some(text) = self
            .service
            .backend
            .read_text()
            .map_err(clipboard_read_failed)?
        {
            let paths = parse_absolute_file_paths(&text);
            Some(if paths.is_empty() {
                TextualClipboardState::Text(text)
            } else {
                TextualClipboardState::Files(paths)
            })
        } else {
            None
        };
        let image = self
            .service
            .backend
            .read_image()
            .map_err(clipboard_read_failed)?
            .map(encode_png)
            .transpose()?;
        let textual_changed = self
            .last_textual_state
            .lock()
            .map_err(|_| runtime_state_failed())?
            .as_ref()
            != textual_state.as_ref();
        let mut inserted = 0;
        let insert_result = (|| {
            if textual_changed {
                inserted += match textual_state.as_ref() {
                    Some(TextualClipboardState::Files(paths)) => {
                        usize::from(self.service.store.insert_files(paths.clone())?)
                    }
                    Some(TextualClipboardState::Text(text)) => {
                        usize::from(self.service.store.insert_text(text)?)
                    }
                    None => 0,
                };
                *self
                    .last_textual_state
                    .lock()
                    .map_err(|_| runtime_state_failed())? = textual_state;
            }
            if let Some(image) = image {
                inserted += usize::from(self.service.store.insert_png_image(
                    &image.bytes,
                    image.width,
                    image.height,
                )?);
            }
            Ok::<(), ServiceError>(())
        })();
        if let Err(error) = insert_result {
            if inserted > 0 {
                let retention_result = self
                    .service
                    .store
                    .enforce_retention(settings.retention_limit);
                self.service.emit_history_changed();
                retention_result?;
            }
            return Err(error);
        }
        let deleted_by_retention = if inserted > 0 {
            self.service
                .store
                .enforce_retention(settings.retention_limit)?
        } else {
            0
        };
        if inserted > 0 || deleted_by_retention > 0 {
            self.service.emit_history_changed();
        }
        Ok(ClipboardRuntimeTick {
            inserted,
            deleted_by_retention,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TextualClipboardState {
    Files(Vec<String>),
    Text(String),
}

fn parse_absolute_file_paths(value: &str) -> Vec<String> {
    let paths = value
        .lines()
        .map(|line| line.trim().trim_start_matches("file://").to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    paths
        .iter()
        .all(|path| Path::new(path).is_absolute())
        .then_some(paths)
        .unwrap_or_default()
}

struct EncodedImage {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
}

fn encode_png(image: ClipboardImage) -> Result<EncodedImage, ServiceError> {
    let rgba = RgbaImage::from_raw(image.width, image.height, image.rgba)
        .ok_or_else(clipboard_image_invalid)?;
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(rgba)
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|_| clipboard_image_invalid())?;
    Ok(EncodedImage {
        bytes: bytes.into_inner(),
        width: image.width,
        height: image.height,
    })
}

fn clipboard_image_invalid() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_CLIPBOARD_IMAGE_INVALID",
        "system clipboard image data is invalid",
    )
}

fn clipboard_read_failed(_error: String) -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_CLIPBOARD_READ_FAILED",
        "cannot read the system clipboard",
    )
}

fn runtime_state_failed() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_RUNTIME_FAILED",
        "clipboard runtime state is unavailable",
    )
}
