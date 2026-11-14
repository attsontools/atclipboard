use std::{
    io::Cursor,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use base64::{
    engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use image::ImageFormat;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::{
    storage::ClipboardSortKey, ClipboardItemKind, ClipboardServiceEvent, ClipboardSettings,
    ClipboardStore, ServiceError,
};

#[derive(Clone)]
pub struct ClipboardService {
    pub(crate) store: Arc<ClipboardStore>,
    pub(crate) backend: Arc<dyn ClipboardBackend>,
    events: broadcast::Sender<ClipboardServiceEvent>,
    revision: Arc<AtomicU64>,
}

pub trait ClipboardBackend: Send + Sync {
    fn read_files(&self) -> Result<Option<Vec<String>>, String> {
        Ok(None)
    }

    fn read_text(&self) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn read_image(&self) -> Result<Option<ClipboardImage>, String> {
        Ok(None)
    }

    fn write_text(&self, value: &str) -> Result<(), String>;
    fn write_image(&self, value: &image::DynamicImage) -> Result<(), String>;

    fn shortcut_status(&self) -> ClipboardShortcutStatus {
        ClipboardShortcutStatus {
            registered: false,
            register_error: None,
            unavailable_reason: Some("global shortcut backend is not configured".into()),
            system_shortcut_command: None,
        }
    }

    fn replace_shortcut(&self, _previous: &str, _next: &str) -> Result<(), String> {
        Err("global shortcut backend is not configured".into())
    }

    fn initialize_shortcut(&self, _shortcut: &str) {}

    fn set_open_history_handler(&self, _handler: Arc<dyn Fn() + Send + Sync>) {}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardShortcutStatus {
    pub registered: bool,
    pub register_error: Option<String>,
    pub unavailable_reason: Option<String>,
    pub system_shortcut_command: Option<String>,
}

struct UnavailableClipboardBackend;

impl ClipboardBackend for UnavailableClipboardBackend {
    fn write_text(&self, _value: &str) -> Result<(), String> {
        Err("clipboard backend is not configured".into())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Err("clipboard backend is not configured".into())
    }
}

impl ClipboardService {
    pub fn open(data_directory: &Path) -> Result<Self, ServiceError> {
        Self::open_with_backend(data_directory, Arc::new(UnavailableClipboardBackend))
    }

    pub fn open_with_backend(
        data_directory: &Path,
        backend: Arc<dyn ClipboardBackend>,
    ) -> Result<Self, ServiceError> {
        let (store, _) = ClipboardStore::open(data_directory)?;
        let (events, _) = broadcast::channel(256);
        Ok(Self {
            store: Arc::new(store),
            backend,
            events,
            revision: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ClipboardServiceEvent> {
        self.events.subscribe()
    }

    pub(crate) fn emit_history_changed(&self) {
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self
            .events
            .send(ClipboardServiceEvent::HistoryChanged { revision });
    }

    pub fn open_history(&self) {
        let _ = self.events.send(ClipboardServiceEvent::HostUiOpen {
            window_id: "history".into(),
            payload: json!({}),
        });
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, ServiceError> {
        match method {
            "clipboard.history.list" => {
                let params: ListParams = decode_params(params)?;
                let limit = params.limit.unwrap_or(50);
                if !(1..=100).contains(&limit) {
                    return Err(invalid_params(
                        "clipboard history limit must be between 1 and 100",
                    ));
                }
                let normalized_query = normalize_query(params.query.as_deref());
                let cursor = params
                    .cursor
                    .as_deref()
                    .map(|value| decode_cursor(value, params.kind, &normalized_query))
                    .transpose()?;
                let mut items = self.store.list_items_after(
                    params.kind,
                    Some(&normalized_query),
                    cursor.as_ref().map(|cursor| &cursor.position),
                    limit + 1,
                )?;
                let next_cursor = if items.len() > limit {
                    items.truncate(limit);
                    items
                        .last()
                        .map(|item| encode_cursor(item, params.kind, &normalized_query))
                        .transpose()?
                } else {
                    None
                };
                let items = items
                    .into_iter()
                    .map(|item| public_item(item, 4 * 1024, 96))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(json!({ "items": items, "nextCursor": next_cursor }))
            }
            "clipboard.history.get" => {
                let params: ItemParams = decode_params(params)?;
                let item = self
                    .store
                    .get_item(&params.id)?
                    .ok_or_else(item_not_found)?;
                Ok(serde_json::to_value(public_item(item, 256 * 1024, 1_600)?)
                    .map_err(|_| response_invalid())?)
            }
            "clipboard.history.pin" => {
                let params: PinParams = decode_params(params)?;
                let item = self
                    .store
                    .get_item(&params.id)?
                    .ok_or_else(item_not_found)?;
                let mut public = public_item(item, 256 * 1024, 1_600)?;
                if public.is_pinned != params.is_pinned {
                    if !self.store.set_pinned(&params.id, params.is_pinned)? {
                        return Err(item_not_found());
                    }
                    self.emit_history_changed();
                }
                public.is_pinned = params.is_pinned;
                Ok(serde_json::to_value(public).map_err(|_| response_invalid())?)
            }
            "clipboard.history.delete" => {
                let params: ItemParams = decode_params(params)?;
                if !self.store.delete_item(&params.id)? {
                    return Err(item_not_found());
                }
                self.emit_history_changed();
                Ok(json!({ "deleted": true }))
            }
            "clipboard.history.clearUnpinned" => {
                let _: EmptyParams = decode_params(params)?;
                let deleted_count = self.store.clear_unpinned()?;
                if deleted_count > 0 {
                    self.emit_history_changed();
                }
                Ok(json!({ "deletedCount": deleted_count }))
            }
            "clipboard.history.restore" => {
                let params: ItemParams = decode_params(params)?;
                let item = self
                    .store
                    .get_item(&params.id)?
                    .ok_or_else(item_not_found)?;
                let write_result = match item.kind {
                    ClipboardItemKind::Text => self.backend.write_text(&item.content_text),
                    ClipboardItemKind::Files => {
                        self.backend.write_text(&item.file_paths.join("\n"))
                    }
                    ClipboardItemKind::Image => {
                        let path = item.asset_path().ok_or_else(|| {
                            ServiceError::new(
                                "CLIPBOARD_SERVICE_DATA_INVALID",
                                "clipboard image asset is missing",
                            )
                        })?;
                        let image = image::open(path).map_err(|_| {
                            ServiceError::new(
                                "CLIPBOARD_SERVICE_DATA_INVALID",
                                "clipboard image asset cannot be decoded",
                            )
                        })?;
                        self.backend.write_image(&image)
                    }
                };
                write_result.map_err(|_| clipboard_write_failed())?;
                let last_copied_at = self
                    .store
                    .touch_copied(&params.id)?
                    .ok_or_else(item_not_found)?;
                self.emit_history_changed();
                Ok(json!({
                    "restored": true,
                    "lastCopiedAt": last_copied_at,
                }))
            }
            "clipboard.settings.get" => {
                let _: EmptyParams = decode_params(params)?;
                settings_response(self.store.load_settings()?, self.backend.shortcut_status())
            }
            "clipboard.settings.update" => {
                let params: SettingsUpdateParams = decode_params(params)?;
                let mut next = params.settings;
                next.shortcut = next.shortcut.trim().to_string();
                validate_settings(&next)?;
                let previous = self.store.load_settings()?;
                let shortcut_changed = previous.shortcut != next.shortcut;
                if shortcut_changed {
                    self.backend
                        .replace_shortcut(&previous.shortcut, &next.shortcut)
                        .map_err(|_| shortcut_register_failed())?;
                }
                if let Err(error) = self.store.save_settings(&next) {
                    rollback_settings(
                        &self.store,
                        &*self.backend,
                        &previous,
                        &next,
                        shortcut_changed,
                        false,
                    )?;
                    return Err(error);
                }
                if let Err(error) = self.store.enforce_retention(next.retention_limit) {
                    rollback_settings(
                        &self.store,
                        &*self.backend,
                        &previous,
                        &next,
                        shortcut_changed,
                        true,
                    )?;
                    return Err(error);
                }
                if previous != next {
                    self.emit_history_changed();
                }
                settings_response(next, self.backend.shortcut_status())
            }
            _ => Err(ServiceError::new(
                "CLIPBOARD_SERVICE_METHOD_NOT_FOUND",
                "clipboard service method is not available",
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListParams {
    kind: Option<ClipboardItemKind>,
    query: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ItemParams {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PinParams {
    id: String,
    is_pinned: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsUpdateParams {
    settings: ClipboardSettings,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListCursor {
    version: u8,
    kind: Option<ClipboardItemKind>,
    query_fingerprint: String,
    #[serde(flatten)]
    position: CursorPosition,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorPosition {
    is_pinned: bool,
    created_at: String,
    id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicClipboardItem {
    id: String,
    kind: ClipboardItemKind,
    preview: String,
    content_text: String,
    content_truncated: bool,
    file_paths: Vec<String>,
    preview_data_url: Option<String>,
    is_pinned: bool,
    created_at: String,
    last_copied_at: Option<String>,
}

fn public_item(
    item: crate::ClipboardHistoryItem,
    text_limit: usize,
    image_dimension: u32,
) -> Result<PublicClipboardItem, ServiceError> {
    let preview_data_url = item
        .asset_path()
        .map(|path| bounded_image_data_url(path, image_dimension))
        .transpose()?;
    let (content_text, content_truncated) = truncate_utf8(&item.content_text, text_limit);
    Ok(PublicClipboardItem {
        id: item.id,
        kind: item.kind,
        preview: item.preview,
        content_text,
        content_truncated,
        file_paths: item.file_paths,
        preview_data_url,
        is_pinned: item.is_pinned,
        created_at: item.created_at,
        last_copied_at: item.last_copied_at,
    })
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

fn decode_params<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, ServiceError> {
    serde_json::from_value(params)
        .map_err(|_| invalid_params("clipboard service params are invalid"))
}

fn invalid_params(message: impl Into<String>) -> ServiceError {
    ServiceError::new("CLIPBOARD_SERVICE_INVALID_PARAMS", message)
}

fn item_not_found() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_ITEM_NOT_FOUND",
        "clipboard history item does not exist",
    )
}

fn response_invalid() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_RESPONSE_INVALID",
        "cannot encode clipboard service response",
    )
}

fn clipboard_write_failed() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_CLIPBOARD_WRITE_FAILED",
        "cannot write clipboard history item",
    )
}

fn shortcut_register_failed() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_SHORTCUT_REGISTER_FAILED",
        "cannot register clipboard history shortcut",
    )
}

fn settings_rollback_failed() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_SETTINGS_ROLLBACK_FAILED",
        "cannot restore previous clipboard settings",
    )
}

fn rollback_settings(
    store: &ClipboardStore,
    backend: &dyn ClipboardBackend,
    previous: &ClipboardSettings,
    next: &ClipboardSettings,
    shortcut_changed: bool,
    restore_database: bool,
) -> Result<(), ServiceError> {
    let database_restored = !restore_database || store.save_settings(previous).is_ok();
    let shortcut_restored = !shortcut_changed
        || backend
            .replace_shortcut(&next.shortcut, &previous.shortcut)
            .is_ok();
    if !database_restored || !shortcut_restored {
        return Err(settings_rollback_failed());
    }
    Ok(())
}

fn validate_settings(settings: &ClipboardSettings) -> Result<(), ServiceError> {
    if !(50..=5_000).contains(&settings.retention_limit) {
        return Err(invalid_params(
            "clipboard retention limit must be between 50 and 5000",
        ));
    }
    if settings.shortcut.is_empty() || settings.shortcut.len() > 256 {
        return Err(invalid_params("clipboard shortcut is invalid"));
    }
    Ok(())
}

fn settings_response(
    settings: ClipboardSettings,
    status: ClipboardShortcutStatus,
) -> Result<Value, ServiceError> {
    Ok(json!({
        "captureEnabled": settings.capture_enabled,
        "retentionLimit": settings.retention_limit,
        "shortcut": settings.shortcut,
        "shortcutRegistered": status.registered,
        "shortcutRegisterError": status.register_error,
        "unavailableReason": status.unavailable_reason,
        "systemShortcutCommand": status.system_shortcut_command,
    }))
}

fn normalize_query(query: Option<&str>) -> String {
    query.unwrap_or("").trim().to_lowercase()
}

fn encode_cursor(
    item: &crate::ClipboardHistoryItem,
    kind: Option<ClipboardItemKind>,
    query: &str,
) -> Result<String, ServiceError> {
    let key = item.sort_key();
    let cursor = ListCursor {
        version: 1,
        kind,
        query_fingerprint: query_fingerprint(query),
        position: CursorPosition {
            is_pinned: key.is_pinned,
            created_at: key.created_at,
            id: key.id,
        },
    };
    serde_json::to_vec(&cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| {
            ServiceError::new(
                "CLIPBOARD_SERVICE_RESPONSE_INVALID",
                "cannot encode list cursor",
            )
        })
}

fn decode_cursor(
    value: &str,
    kind: Option<ClipboardItemKind>,
    query: &str,
) -> Result<ListCursorDecoded, ServiceError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| invalid_params("clipboard history cursor is invalid"))?;
    let cursor: ListCursor = serde_json::from_slice(&bytes)
        .map_err(|_| invalid_params("clipboard history cursor is invalid"))?;
    if cursor.version != 1
        || cursor.kind != kind
        || cursor.query_fingerprint != query_fingerprint(query)
        || cursor.position.id.is_empty()
        || cursor.position.created_at.is_empty()
    {
        return Err(invalid_params("clipboard history cursor is invalid"));
    }
    Ok(ListCursorDecoded {
        position: ClipboardSortKey {
            is_pinned: cursor.position.is_pinned,
            created_at: cursor.position.created_at,
            id: cursor.position.id,
        },
    })
}

fn query_fingerprint(query: &str) -> String {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let hash = query.as_bytes().iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    });
    format!("{hash:016x}")
}

struct ListCursorDecoded {
    position: ClipboardSortKey,
}

fn bounded_image_data_url(path: &Path, max_dimension: u32) -> Result<String, ServiceError> {
    const MAX_DATA_URL_BYTES: usize = 2 * 1024 * 1024;
    const PREFIX: &str = "data:image/png;base64,";

    let source = image::open(path).map_err(|_| {
        ServiceError::new(
            "CLIPBOARD_SERVICE_DATA_INVALID",
            "clipboard image asset cannot be decoded",
        )
    })?;
    let mut width = source.width().min(max_dimension).max(1);
    let mut height = source.height().min(max_dimension).max(1);
    loop {
        let resized = source.thumbnail(width, height);
        let mut encoded = Cursor::new(Vec::new());
        resized
            .write_to(&mut encoded, ImageFormat::Png)
            .map_err(|_| response_invalid())?;
        let data_url = format!("{PREFIX}{}", BASE64_STANDARD.encode(encoded.into_inner()));
        if data_url.len() <= MAX_DATA_URL_BYTES {
            return Ok(data_url);
        }
        if width == 1 && height == 1 {
            return Err(response_invalid());
        }
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
}
