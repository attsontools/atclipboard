use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use chrono::Utc;
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::ServiceError;

const APP_ID: &str = "com.attson.clipboard";
const DATABASE_NAME: &str = "clipboard_history.sqlite3";
const CAPABILITY_NAME: &str = "clipboard-takeover-capability-v1";
const MARKER_NAME: &str = "clipboard-takeover.json";
const OWNER_LOCK_NAME: &str = "clipboard-owner.lock";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardItemKind {
    Text,
    Image,
    Files,
}

impl ClipboardItemKind {
    fn from_database(value: &str) -> Result<Self, ServiceError> {
        match value {
            "text" => Ok(Self::Text),
            "image" => Ok(Self::Image),
            "files" => Ok(Self::Files),
            _ => Err(data_error("clipboard item kind is invalid")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardHistoryItem {
    pub id: String,
    pub kind: ClipboardItemKind,
    pub preview: String,
    pub content_text: String,
    pub file_paths: Vec<String>,
    pub is_pinned: bool,
    pub created_at: String,
    pub last_copied_at: Option<String>,
    asset_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClipboardSortKey {
    pub is_pinned: bool,
    pub created_at: String,
    pub id: String,
}

impl ClipboardHistoryItem {
    pub(crate) fn sort_key(&self) -> ClipboardSortKey {
        ClipboardSortKey {
            is_pinned: self.is_pinned,
            created_at: self.created_at.clone(),
            id: self.id.clone(),
        }
    }

    pub(crate) fn asset_path(&self) -> Option<&Path> {
        self.asset_path.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TakeoverOutcome {
    Created,
    TakenOver { items: u64, assets: u64 },
    ExistingOwned { items: u64, assets: u64 },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipboardSettings {
    pub capture_enabled: bool,
    pub retention_limit: usize,
    pub shortcut: String,
}

pub struct ClipboardStore {
    root_dir: PathBuf,
    db_path: PathBuf,
    _owner: OwnerLease,
}

impl ClipboardStore {
    pub fn open(root_dir: &Path) -> Result<(Self, TakeoverOutcome), ServiceError> {
        fs::create_dir_all(root_dir).map_err(storage_io_error)?;
        let owner = OwnerLease::acquire(root_dir)?;
        let db_path = root_dir.join(DATABASE_NAME);
        if marker_state(root_dir)?.as_deref() == Some("owned") {
            if !db_path.is_file() {
                return Err(ServiceError::new(
                    "CLIPBOARD_SERVICE_DATA_MISSING",
                    "service owns clipboard history but its database is missing",
                ));
            }
            let (items, assets) = validate_database(root_dir, &db_path)?;
            return Ok((
                Self {
                    root_dir: root_dir.to_path_buf(),
                    db_path,
                    _owner: owner,
                },
                TakeoverOutcome::ExistingOwned { items, assets },
            ));
        }
        let database_existed = db_path.is_file();
        if database_existed && !root_dir.join(CAPABILITY_NAME).is_file() {
            return Err(ServiceError::new(
                "CLIPBOARD_SERVICE_HOST_UPGRADE_REQUIRED",
                "run a takeover-capable AT Tool version before claiming clipboard data",
            ));
        }

        let nonce = Uuid::new_v4().to_string();
        write_marker(root_dir, "migrating", &nonce)?;
        if !database_existed {
            if let Err(error) = fs::create_dir_all(root_dir.join("assets"))
                .map_err(storage_io_error)
                .and_then(|_| init_database(&db_path))
            {
                remove_marker_if_nonce(root_dir, &nonce);
                let _ = fs::remove_file(&db_path);
                return Err(error);
            }
        }
        let validated = validate_database(root_dir, &db_path);
        let (items, assets) = match validated {
            Ok(counts) => counts,
            Err(error) => {
                remove_marker_if_nonce(root_dir, &nonce);
                if !database_existed {
                    let _ = fs::remove_file(&db_path);
                }
                return Err(error);
            }
        };
        write_marker(root_dir, "owned", &nonce)?;
        Ok((
            Self {
                root_dir: root_dir.to_path_buf(),
                db_path,
                _owner: owner,
            },
            if database_existed {
                TakeoverOutcome::TakenOver { items, assets }
            } else {
                TakeoverOutcome::Created
            },
        ))
    }

    pub fn list_items(
        &self,
        kind: Option<ClipboardItemKind>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ClipboardHistoryItem>, ServiceError> {
        self.list_items_after(kind, query, None, limit)
    }

    pub fn get_item(&self, id: &str) -> Result<Option<ClipboardHistoryItem>, ServiceError> {
        let connection = Connection::open(&self.db_path).map_err(storage_sql_error)?;
        let row = connection
            .query_row(
                r#"
                SELECT id, kind, preview, content_text, asset_path, file_paths_json,
                       is_pinned, created_at, last_copied_at
                FROM clipboard_items
                WHERE id = ?1
                "#,
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_sql_error)?;
        row.map(history_item_from_row).transpose()
    }

    pub(crate) fn list_items_after(
        &self,
        kind: Option<ClipboardItemKind>,
        query: Option<&str>,
        cursor: Option<&ClipboardSortKey>,
        limit: usize,
    ) -> Result<Vec<ClipboardHistoryItem>, ServiceError> {
        let connection = Connection::open(&self.db_path).map_err(storage_sql_error)?;
        let mut statement = connection
            .prepare(
                r#"
                SELECT id, kind, preview, content_text, asset_path, file_paths_json,
                       is_pinned, created_at, last_copied_at
                FROM clipboard_items
                ORDER BY is_pinned DESC, created_at DESC, id DESC
                "#,
            )
            .map_err(storage_sql_error)?;
        let query = query.unwrap_or("").trim().to_lowercase();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map_err(storage_sql_error)?;
        let mut items = Vec::new();
        for row in rows {
            let (
                id,
                kind_value,
                preview,
                content_text,
                asset_path,
                file_paths_json,
                is_pinned,
                created_at,
                last_copied_at,
            ) = row.map_err(storage_sql_error)?;
            let item_kind = ClipboardItemKind::from_database(&kind_value)?;
            let file_paths: Vec<String> =
                serde_json::from_str(&file_paths_json).map_err(storage_json_error)?;
            if kind.is_some_and(|expected| expected != item_kind) {
                continue;
            }
            if !query.is_empty()
                && !format!("{preview}\n{content_text}\n{}", file_paths.join("\n"))
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            let row_key = ClipboardSortKey {
                is_pinned: is_pinned == 1,
                created_at: created_at.clone(),
                id: id.clone(),
            };
            if cursor.is_some_and(|cursor| !sorts_after(&row_key, cursor)) {
                continue;
            }
            items.push(ClipboardHistoryItem {
                id,
                kind: item_kind,
                preview,
                content_text,
                file_paths,
                is_pinned: is_pinned == 1,
                created_at,
                last_copied_at,
                asset_path: asset_path.map(PathBuf::from),
            });
            if items.len() >= limit.max(1) {
                break;
            }
        }
        Ok(items)
    }

    pub fn set_pinned(&self, id: &str, is_pinned: bool) -> Result<bool, ServiceError> {
        let connection = Connection::open(&self.db_path).map_err(storage_sql_error)?;
        let changed = connection
            .execute(
                "UPDATE clipboard_items SET is_pinned = ?2 WHERE id = ?1",
                params![id, i64::from(is_pinned)],
            )
            .map_err(storage_sql_error)?;
        Ok(changed == 1)
    }

    pub fn delete_item(&self, id: &str) -> Result<bool, ServiceError> {
        let Some(item) = self.get_item(id)? else {
            return Ok(false);
        };
        self.delete_items(&[item])?;
        Ok(true)
    }

    pub fn clear_unpinned(&self) -> Result<usize, ServiceError> {
        let items = self
            .list_items_after(None, None, None, usize::MAX)?
            .into_iter()
            .filter(|item| !item.is_pinned)
            .collect::<Vec<_>>();
        self.delete_items(&items)?;
        Ok(items.len())
    }

    pub fn touch_copied(&self, id: &str) -> Result<Option<String>, ServiceError> {
        let copied_at = Utc::now().to_rfc3339();
        let connection = Connection::open(&self.db_path).map_err(storage_sql_error)?;
        let changed = connection
            .execute(
                "UPDATE clipboard_items SET last_copied_at = ?2 WHERE id = ?1",
                params![id, copied_at],
            )
            .map_err(storage_sql_error)?;
        Ok((changed == 1).then_some(copied_at))
    }

    pub fn load_settings(&self) -> Result<ClipboardSettings, ServiceError> {
        let connection = Connection::open(&self.db_path).map_err(storage_sql_error)?;
        let setting = |key: &str| -> Result<String, ServiceError> {
            connection
                .query_row(
                    "SELECT value FROM clipboard_settings WHERE key = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_sql_error)?
                .ok_or_else(|| data_error("clipboard settings are incomplete"))
        };
        let capture_enabled = match setting("capture_enabled")?.as_str() {
            "1" => true,
            "0" => false,
            _ => return Err(data_error("clipboard capture setting is invalid")),
        };
        let retention_limit = setting("retention_limit")?
            .parse::<usize>()
            .map_err(|_| data_error("clipboard retention setting is invalid"))?;
        let shortcut = setting("shortcut")?;
        if shortcut.trim().is_empty() {
            return Err(data_error("clipboard shortcut setting is invalid"));
        }
        Ok(ClipboardSettings {
            capture_enabled,
            retention_limit,
            shortcut,
        })
    }

    pub fn save_settings(&self, settings: &ClipboardSettings) -> Result<(), ServiceError> {
        let mut connection = Connection::open(&self.db_path).map_err(storage_sql_error)?;
        let transaction = connection.transaction().map_err(storage_sql_error)?;
        for (key, value) in [
            (
                "capture_enabled",
                if settings.capture_enabled { "1" } else { "0" }.to_string(),
            ),
            ("retention_limit", settings.retention_limit.to_string()),
            ("shortcut", settings.shortcut.clone()),
        ] {
            transaction
                .execute(
                    r#"
                    INSERT INTO clipboard_settings (key, value) VALUES (?1, ?2)
                    ON CONFLICT(key) DO UPDATE SET value = excluded.value
                    "#,
                    params![key, value],
                )
                .map_err(storage_sql_error)?;
        }
        transaction.commit().map_err(storage_sql_error)
    }

    pub fn enforce_retention(&self, limit: usize) -> Result<usize, ServiceError> {
        let items = self.list_items_after(None, None, None, usize::MAX)?;
        let pinned_count = items.iter().filter(|item| item.is_pinned).count();
        let unpinned_limit = limit.saturating_sub(pinned_count);
        let items = items
            .into_iter()
            .filter(|item| !item.is_pinned)
            .skip(unpinned_limit)
            .collect::<Vec<_>>();
        self.delete_items(&items)?;
        Ok(items.len())
    }

    fn delete_items(&self, items: &[ClipboardHistoryItem]) -> Result<(), ServiceError> {
        let assets = items
            .iter()
            .filter_map(ClipboardHistoryItem::asset_path)
            .map(|path| self.validated_asset_path(path))
            .collect::<Result<Vec<_>, _>>()?;
        let mut connection = Connection::open(&self.db_path).map_err(storage_sql_error)?;
        let transaction = connection.transaction().map_err(storage_sql_error)?;
        for item in items {
            let changed = transaction
                .execute(
                    "DELETE FROM clipboard_items WHERE id = ?1",
                    params![item.id],
                )
                .map_err(storage_sql_error)?;
            if changed != 1 {
                return Err(storage_changed());
            }
        }
        transaction.commit().map_err(storage_sql_error)?;
        for asset in assets {
            if fs::remove_file(asset).is_err() {
                eprintln!(
                    "CLIPBOARD_SERVICE_ASSET_CLEANUP_FAILED: deleted history has an orphaned image asset"
                );
            }
        }
        Ok(())
    }

    fn validated_asset_path(&self, path: &Path) -> Result<PathBuf, ServiceError> {
        let assets = self
            .root_dir
            .join("assets")
            .canonicalize()
            .map_err(|_| data_error("clipboard asset directory is invalid"))?;
        let asset = path
            .canonicalize()
            .map_err(|_| data_error("clipboard image asset is missing"))?;
        if !asset.starts_with(assets) || !asset.is_file() {
            return Err(data_error(
                "clipboard image asset is outside the asset directory",
            ));
        }
        Ok(asset)
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }
}

fn sorts_after(candidate: &ClipboardSortKey, cursor: &ClipboardSortKey) -> bool {
    candidate.is_pinned < cursor.is_pinned
        || (candidate.is_pinned == cursor.is_pinned
            && (candidate.created_at < cursor.created_at
                || (candidate.created_at == cursor.created_at && candidate.id < cursor.id)))
}

type ClipboardItemRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    i64,
    String,
    Option<String>,
);

fn history_item_from_row(row: ClipboardItemRow) -> Result<ClipboardHistoryItem, ServiceError> {
    let (
        id,
        kind,
        preview,
        content_text,
        asset_path,
        file_paths_json,
        is_pinned,
        created_at,
        last_copied_at,
    ) = row;
    Ok(ClipboardHistoryItem {
        id,
        kind: ClipboardItemKind::from_database(&kind)?,
        preview,
        content_text,
        file_paths: serde_json::from_str(&file_paths_json).map_err(storage_json_error)?,
        is_pinned: is_pinned == 1,
        created_at,
        last_copied_at,
        asset_path: asset_path.map(PathBuf::from),
    })
}

fn init_database(path: &Path) -> Result<(), ServiceError> {
    let connection = Connection::open(path).map_err(storage_sql_error)?;
    connection
        .execute_batch(
            r#"
            CREATE TABLE clipboard_items (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                preview TEXT NOT NULL,
                content_text TEXT NOT NULL,
                asset_path TEXT,
                file_paths_json TEXT NOT NULL,
                content_hash TEXT NOT NULL UNIQUE,
                is_pinned INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                last_copied_at TEXT
            );
            CREATE INDEX idx_clipboard_items_created_at
                ON clipboard_items(created_at DESC);
            CREATE INDEX idx_clipboard_items_kind ON clipboard_items(kind);
            CREATE TABLE clipboard_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO clipboard_settings VALUES ('capture_enabled', '1');
            INSERT INTO clipboard_settings VALUES ('retention_limit', '500');
            INSERT INTO clipboard_settings VALUES ('shortcut', 'CommandOrControl+Alt+V');
            "#,
        )
        .map_err(storage_sql_error)
}

struct OwnerLease {
    file: File,
}

impl OwnerLease {
    fn acquire(root_dir: &Path) -> Result<Self, ServiceError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(root_dir.join(OWNER_LOCK_NAME))
            .map_err(storage_io_error)?;
        file.try_lock_exclusive().map_err(|_| {
            ServiceError::new(
                "CLIPBOARD_SERVICE_ALREADY_RUNNING",
                "clipboard data already has an active owner",
            )
        })?;
        Ok(Self { file })
    }
}

impl Drop for OwnerLease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn validate_database(root_dir: &Path, db_path: &Path) -> Result<(u64, u64), ServiceError> {
    let connection = Connection::open(db_path).map_err(storage_sql_error)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(storage_sql_error)?;
    if integrity != "ok" {
        return Err(data_error("clipboard database failed integrity check"));
    }
    for table in ["clipboard_items", "clipboard_settings"] {
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![table],
                |row| row.get(0),
            )
            .map_err(storage_sql_error)?;
        if exists != 1 {
            return Err(data_error("clipboard database schema is incomplete"));
        }
    }
    validate_table_columns(
        &connection,
        "clipboard_items",
        &[
            "id",
            "kind",
            "preview",
            "content_text",
            "asset_path",
            "file_paths_json",
            "content_hash",
            "is_pinned",
            "created_at",
            "last_copied_at",
        ],
    )?;
    validate_table_columns(&connection, "clipboard_settings", &["key", "value"])?;
    let setting = |key: &str| -> Result<String, ServiceError> {
        connection
            .query_row(
                "SELECT value FROM clipboard_settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_sql_error)?
            .ok_or_else(|| data_error(format!("clipboard setting {key} is missing")))
    };
    if !matches!(setting("capture_enabled")?.as_str(), "0" | "1") {
        return Err(data_error("clipboard capture_enabled setting is invalid"));
    }
    if setting("retention_limit")?
        .parse::<usize>()
        .ok()
        .filter(|limit| *limit > 0)
        .is_none()
    {
        return Err(data_error("clipboard retention_limit setting is invalid"));
    }
    if setting("shortcut")?.trim().is_empty() {
        return Err(data_error("clipboard shortcut setting is invalid"));
    }
    let root = root_dir.canonicalize().map_err(storage_io_error)?;
    let assets_root = root_dir
        .join("assets")
        .canonicalize()
        .map_err(storage_io_error)?;
    if !assets_root.starts_with(&root) {
        return Err(data_error("clipboard asset directory is invalid"));
    }
    let mut statement = connection
        .prepare("SELECT kind, asset_path, file_paths_json FROM clipboard_items")
        .map_err(storage_sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(storage_sql_error)?;
    let mut items = 0_u64;
    let mut assets = 0_u64;
    for row in rows {
        let (kind, asset_path, file_paths_json) = row.map_err(storage_sql_error)?;
        let kind = ClipboardItemKind::from_database(&kind)?;
        serde_json::from_str::<Vec<String>>(&file_paths_json).map_err(storage_json_error)?;
        match (kind, asset_path) {
            (ClipboardItemKind::Image, Some(path)) => {
                let asset = PathBuf::from(path)
                    .canonicalize()
                    .map_err(|_| data_error("clipboard image asset is missing"))?;
                if !asset.starts_with(&assets_root) || !asset.is_file() {
                    return Err(data_error(
                        "clipboard image asset is outside the asset directory",
                    ));
                }
                image::ImageReader::open(&asset)
                    .map_err(|_| data_error("clipboard image asset is invalid"))?
                    .with_guessed_format()
                    .map_err(|_| data_error("clipboard image asset is invalid"))?
                    .decode()
                    .map_err(|_| data_error("clipboard image asset is invalid"))?;
                assets += 1;
            }
            (ClipboardItemKind::Image, None) => {
                return Err(data_error("clipboard image asset is missing"));
            }
            (_, Some(_)) => return Err(data_error("non-image clipboard item has an asset path")),
            (_, None) => {}
        }
        items += 1;
    }
    Ok((items, assets))
}

fn validate_table_columns(
    connection: &Connection,
    table: &str,
    required: &[&str],
) -> Result<(), ServiceError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(storage_sql_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(storage_sql_error)?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(storage_sql_error)?;
    if required.iter().any(|column| !columns.contains(*column)) {
        return Err(data_error("clipboard database schema is incompatible"));
    }
    Ok(())
}

fn write_marker(root_dir: &Path, state: &str, nonce: &str) -> Result<(), ServiceError> {
    let destination = root_dir.join(MARKER_NAME);
    let temporary = root_dir.join(format!("{MARKER_NAME}.tmp-{}", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(&json!({
        "protocolVersion": 1,
        "appId": APP_ID,
        "schemaVersion": 1,
        "state": state,
        "nonce": nonce
    }))
    .map_err(storage_json_error)?;
    let mut file = File::create(&temporary).map_err(storage_io_error)?;
    file.write_all(&bytes).map_err(storage_io_error)?;
    file.sync_all().map_err(storage_io_error)?;
    replace_file(&temporary, &destination).map_err(storage_io_error)?;
    if let Ok(directory) = File::open(root_dir) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn marker_state(root_dir: &Path) -> Result<Option<String>, ServiceError> {
    let path = root_dir.join(MARKER_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let marker: Value = serde_json::from_slice(&fs::read(path).map_err(storage_io_error)?)
        .map_err(storage_json_error)?;
    if marker.get("appId").and_then(Value::as_str) != Some(APP_ID)
        || marker.get("protocolVersion").and_then(Value::as_u64) != Some(1)
        || marker.get("schemaVersion").and_then(Value::as_u64) != Some(1)
        || marker
            .get("nonce")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(data_error("clipboard takeover marker is invalid"));
    }
    marker
        .get("state")
        .and_then(Value::as_str)
        .filter(|state| matches!(*state, "migrating" | "owned"))
        .map(|state| Some(state.to_string()))
        .ok_or_else(|| data_error("clipboard takeover marker is invalid"))
}

fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(source, destination)
}

fn remove_marker_if_nonce(root_dir: &Path, nonce: &str) {
    let path = root_dir.join(MARKER_NAME);
    let matches = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|marker| marker.get("nonce")?.as_str().map(str::to_owned))
        .as_deref()
        == Some(nonce);
    if matches {
        let _ = fs::remove_file(path);
    }
}

fn data_error(message: impl Into<String>) -> ServiceError {
    ServiceError::new("CLIPBOARD_SERVICE_DATA_INVALID", message)
}

fn storage_io_error(error: std::io::Error) -> ServiceError {
    ServiceError::new("CLIPBOARD_SERVICE_STORAGE_FAILED", error.to_string())
}

fn storage_sql_error(error: rusqlite::Error) -> ServiceError {
    ServiceError::new("CLIPBOARD_SERVICE_STORAGE_FAILED", error.to_string())
}

fn storage_json_error(error: serde_json::Error) -> ServiceError {
    ServiceError::new("CLIPBOARD_SERVICE_DATA_INVALID", error.to_string())
}

fn storage_changed() -> ServiceError {
    ServiceError::new(
        "CLIPBOARD_SERVICE_STORAGE_CHANGED",
        "clipboard history changed during the operation",
    )
}
