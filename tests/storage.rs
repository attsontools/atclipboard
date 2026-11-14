use std::fs::{self, OpenOptions};

use atclipboard_service::{ClipboardItemKind, ClipboardStore, ServiceError, TakeoverOutcome};
use fs2::FileExt;
use image::{Rgba, RgbaImage};
use rusqlite::{params, Connection};

#[test]
fn existing_history_is_claimed_in_place_without_rewriting_user_data() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    let database_before = fs::read(data.join("clipboard_history.sqlite3")).unwrap();
    let asset_before = fs::read(data.join("assets/clip-image.png")).unwrap();

    let (store, outcome) = ClipboardStore::open(&data).unwrap();

    assert_eq!(
        outcome,
        TakeoverOutcome::TakenOver {
            items: 2,
            assets: 1
        }
    );
    assert_eq!(marker_state(&data), "owned");
    assert_eq!(
        fs::read(data.join("clipboard_history.sqlite3")).unwrap(),
        database_before
    );
    assert_eq!(
        fs::read(data.join("assets/clip-image.png")).unwrap(),
        asset_before
    );
    let items = store.list_items(None, None, 100).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].kind, ClipboardItemKind::Image);
    assert!(items[0].is_pinned);
    assert_eq!(items[1].content_text, "hello clipboard");
}

#[test]
fn owned_history_reopens_without_host_capability_or_a_second_takeover() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    let (store, _) = ClipboardStore::open(&data).unwrap();
    drop(store);
    fs::remove_file(data.join("clipboard-takeover-capability-v1")).unwrap();

    let (store, outcome) = ClipboardStore::open(&data).unwrap();

    assert_eq!(
        outcome,
        TakeoverOutcome::ExistingOwned {
            items: 2,
            assets: 1
        }
    );
    assert_eq!(marker_state(&data), "owned");
    assert_eq!(store.list_items(None, None, 100).unwrap().len(), 2);
}

#[test]
fn a_fresh_directory_is_initialized_and_owned_without_host_capability() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");

    let (store, outcome) = ClipboardStore::open(&data).unwrap();

    assert_eq!(outcome, TakeoverOutcome::Created);
    assert_eq!(marker_state(&data), "owned");
    assert!(data.join("clipboard_history.sqlite3").is_file());
    assert!(data.join("assets").is_dir());
    assert!(store.list_items(None, None, 100).unwrap().is_empty());
}

#[test]
fn takeover_rejects_incomplete_settings_without_claiming_the_data() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute("DELETE FROM clipboard_settings WHERE key = 'shortcut'", [])
        .unwrap();
    drop(connection);

    let error = match ClipboardStore::open(&data) {
        Ok(_) => panic!("incomplete settings must not be claimed"),
        Err(error) => error,
    };

    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");
    assert!(!data.join("clipboard-takeover.json").exists());
}

#[test]
fn takeover_rejects_an_incompatible_item_schema_without_claiming_the_data() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    fs::create_dir_all(data.join("assets")).unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
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
                created_at TEXT NOT NULL
            );
            CREATE TABLE clipboard_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO clipboard_settings VALUES ('capture_enabled', '1');
            INSERT INTO clipboard_settings VALUES ('retention_limit', '500');
            INSERT INTO clipboard_settings VALUES ('shortcut', 'CommandOrControl+Alt+V');
            "#,
        )
        .unwrap();
    drop(connection);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();

    let error = match ClipboardStore::open(&data) {
        Ok(_) => panic!("incompatible schema must not be claimed"),
        Err(error) => error,
    };

    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");
    assert!(!data.join("clipboard-takeover.json").exists());
}

#[test]
fn takeover_requires_a_capable_host_and_preserves_legacy_data() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    let database_before = fs::read(data.join("clipboard_history.sqlite3")).unwrap();

    let error = open_error(&data);

    assert_eq!(error.code, "CLIPBOARD_SERVICE_HOST_UPGRADE_REQUIRED");
    assert!(!data.join("clipboard-takeover.json").exists());
    assert_eq!(
        fs::read(data.join("clipboard_history.sqlite3")).unwrap(),
        database_before
    );
}

#[test]
fn active_legacy_owner_prevents_service_takeover() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    let owner = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(data.join("clipboard-owner.lock"))
        .unwrap();
    owner.lock_exclusive().unwrap();

    let error = open_error(&data);

    assert_eq!(error.code, "CLIPBOARD_SERVICE_ALREADY_RUNNING");
    assert!(!data.join("clipboard-takeover.json").exists());
}

#[test]
fn stale_migrating_marker_is_recovered_and_claimed() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    write_marker(&data, "migrating");

    let (store, outcome) = ClipboardStore::open(&data).unwrap();

    assert_eq!(
        outcome,
        TakeoverOutcome::TakenOver {
            items: 2,
            assets: 1
        }
    );
    assert_eq!(marker_state(&data), "owned");
    assert_eq!(store.list_items(None, None, 100).unwrap().len(), 2);
}

#[test]
fn takeover_rejects_a_missing_image_asset() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    fs::remove_file(data.join("assets/clip-image.png")).unwrap();

    let error = open_error(&data);

    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");
    assert!(!data.join("clipboard-takeover.json").exists());
}

#[test]
fn takeover_rejects_an_image_asset_outside_the_asset_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    let outside = temporary.path().join("outside.png");
    RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255]))
        .save(&outside)
        .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'clip-image'",
            params![outside.to_string_lossy()],
        )
        .unwrap();
    drop(connection);

    let error = open_error(&data);

    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");
    assert!(!data.join("clipboard-takeover.json").exists());
}

#[test]
fn takeover_rejects_an_undecodable_image_asset() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    fs::write(data.join("assets/clip-image.png"), b"not an image").unwrap();

    let error = open_error(&data);

    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");
    assert!(!data.join("clipboard-takeover.json").exists());
}

#[test]
fn owned_marker_without_a_nonce_is_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_legacy_data(&data);
    fs::write(
        data.join("clipboard-takeover.json"),
        br#"{"protocolVersion":1,"appId":"com.attson.clipboard","schemaVersion":1,"state":"owned"}"#,
    )
    .unwrap();

    let error = open_error(&data);

    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");
}

fn seed_legacy_data(data: &std::path::Path) {
    fs::create_dir_all(data.join("assets")).unwrap();
    let asset_path = data.join("assets/clip-image.png");
    RgbaImage::from_pixel(2, 2, Rgba([52, 211, 153, 255]))
        .save(&asset_path)
        .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
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
            INSERT INTO clipboard_items VALUES (
                'clip-text', 'text', 'hello clipboard', 'hello clipboard', NULL,
                '[]', 'hash-text', 0, '2026-11-15T10:00:00+08:00', NULL
            );
            INSERT INTO clipboard_settings VALUES ('capture_enabled', '1');
            INSERT INTO clipboard_settings VALUES ('retention_limit', '500');
            INSERT INTO clipboard_settings VALUES ('shortcut', 'CommandOrControl+Alt+V');
            "#,
        )
        .unwrap();
    connection
        .execute(
            r#"
            INSERT INTO clipboard_items VALUES (
                'clip-image', 'image', 'Image 2 x 2', '', ?1,
                '[]', 'hash-image', 1, '2026-11-15T11:00:00+08:00', NULL
            )
            "#,
            params![asset_path.to_string_lossy()],
        )
        .unwrap();
}

fn marker_state(data: &std::path::Path) -> String {
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(data.join("clipboard-takeover.json")).unwrap()).unwrap();
    marker["state"].as_str().unwrap().to_string()
}

fn write_marker(data: &std::path::Path, state: &str) {
    fs::write(
        data.join("clipboard-takeover.json"),
        format!(
            r#"{{"protocolVersion":1,"appId":"com.attson.clipboard","schemaVersion":1,"state":"{state}","nonce":"stale"}}"#
        ),
    )
    .unwrap();
}

fn open_error(data: &std::path::Path) -> ServiceError {
    match ClipboardStore::open(data) {
        Ok(_) => panic!("takeover must fail"),
        Err(error) => error,
    }
}
