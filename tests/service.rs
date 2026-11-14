use std::{
    fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use atclipboard_service::{
    ClipboardBackend, ClipboardService, ClipboardServiceEvent, ClipboardShortcutStatus,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use image::{Rgba, RgbaImage};
use rusqlite::Connection;
use serde_json::{json, Value};

#[tokio::test]
async fn list_uses_stable_opaque_cursor_in_legacy_sort_order() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_history(
        &data,
        &[
            (
                "pinned-b",
                "text",
                "shared",
                true,
                "2026-11-15T11:00:00+08:00",
            ),
            (
                "pinned-a",
                "text",
                "shared",
                true,
                "2026-11-15T11:00:00+08:00",
            ),
            (
                "plain-c",
                "text",
                "shared",
                false,
                "2026-11-15T12:00:00+08:00",
            ),
            (
                "plain-b",
                "text",
                "shared",
                false,
                "2026-11-15T10:00:00+08:00",
            ),
            (
                "plain-a",
                "text",
                "shared",
                false,
                "2026-11-15T10:00:00+08:00",
            ),
        ],
    );
    let service = ClipboardService::open(&data).unwrap();

    let first = service
        .request("clipboard.history.list", json!({ "limit": 2 }))
        .await
        .unwrap();
    assert_eq!(item_ids(&first), ["pinned-b", "pinned-a"]);
    let cursor = first["nextCursor"].as_str().expect("next cursor");
    assert!(!cursor.contains("pinned-a"));

    let second = service
        .request(
            "clipboard.history.list",
            json!({ "limit": 2, "cursor": cursor }),
        )
        .await
        .unwrap();
    assert_eq!(item_ids(&second), ["plain-c", "plain-b"]);
    let cursor = second["nextCursor"].as_str().expect("next cursor");

    let third = service
        .request(
            "clipboard.history.list",
            json!({ "limit": 2, "cursor": cursor }),
        )
        .await
        .unwrap();
    assert_eq!(item_ids(&third), ["plain-a"]);
    assert!(third["nextCursor"].is_null());
}

#[tokio::test]
async fn list_filters_original_content_and_returns_a_bounded_public_dto() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    let long_content = format!("{}tail-marker", "剪".repeat(2_000));
    seed_history(
        &data,
        &[
            (
                "long-text",
                "text",
                &long_content,
                false,
                "2026-11-15T11:00:00+08:00",
            ),
            (
                "other-files",
                "files",
                "tail-marker",
                false,
                "2026-11-15T10:00:00+08:00",
            ),
        ],
    );
    let service = ClipboardService::open(&data).unwrap();

    let result = service
        .request(
            "clipboard.history.list",
            json!({ "kind": "text", "query": "TAIL-MARKER" }),
        )
        .await
        .unwrap();

    assert_eq!(item_ids(&result), ["long-text"]);
    let item = &result["items"][0];
    assert!(item["contentText"].as_str().unwrap().len() <= 4 * 1024);
    assert_eq!(item["contentTruncated"], true);
    assert_eq!(item["previewDataUrl"], Value::Null);
    let mut keys = item
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "contentText",
            "contentTruncated",
            "createdAt",
            "filePaths",
            "id",
            "isPinned",
            "kind",
            "lastCopiedAt",
            "preview",
            "previewDataUrl",
        ]
    );
}

#[tokio::test]
async fn get_bounds_text_and_exposes_images_only_as_bounded_data_urls() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    let long_content = "界".repeat(100_000);
    seed_history(
        &data,
        &[
            (
                "long-text",
                "text",
                &long_content,
                false,
                "2026-11-15T11:00:00+08:00",
            ),
            (
                "large-image",
                "image",
                "Image 1600 x 1600",
                false,
                "2026-11-15T10:00:00+08:00",
            ),
        ],
    );
    let asset_path = data.join("assets/large-image.png");
    RgbaImage::from_fn(1_600, 1_600, |x, y| {
        let value = x
            .wrapping_mul(1_664_525)
            .wrapping_add(y.wrapping_mul(1_013_904_223));
        Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255])
    })
    .save(&asset_path)
    .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'large-image'",
            rusqlite::params![asset_path.to_string_lossy()],
        )
        .unwrap();
    drop(connection);
    let service = ClipboardService::open(&data).unwrap();

    let text = service
        .request("clipboard.history.get", json!({ "id": "long-text" }))
        .await
        .unwrap();
    assert!(text["contentText"].as_str().unwrap().len() <= 256 * 1024);
    assert_eq!(text["contentTruncated"], true);

    let list = service
        .request(
            "clipboard.history.list",
            json!({ "kind": "image", "limit": 1 }),
        )
        .await
        .unwrap();
    let thumbnail = list["items"][0]["previewDataUrl"].as_str().unwrap();
    assert!(thumbnail.starts_with("data:image/png;base64,"));
    assert!(thumbnail.len() <= 2 * 1024 * 1024);

    let image = service
        .request("clipboard.history.get", json!({ "id": "large-image" }))
        .await
        .unwrap();
    let preview = image["previewDataUrl"].as_str().unwrap();
    assert!(preview.starts_with("data:image/png;base64,"));
    assert!(preview.len() <= 2 * 1024 * 1024);
    BASE64_STANDARD
        .decode(preview.strip_prefix("data:image/png;base64,").unwrap())
        .expect("preview must use standard base64");
    assert!(!image
        .to_string()
        .contains(asset_path.to_string_lossy().as_ref()));

    let error = service
        .request("clipboard.history.get", json!({ "id": "missing" }))
        .await
        .unwrap_err();
    assert_eq!(error.code, "CLIPBOARD_SERVICE_ITEM_NOT_FOUND");
}

#[tokio::test]
async fn pin_delete_and_clear_unpinned_preserve_legacy_cleanup_semantics() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_history(
        &data,
        &[
            (
                "keep-pinned",
                "text",
                "keep",
                true,
                "2026-11-15T12:00:00+08:00",
            ),
            (
                "pin-now",
                "text",
                "pin me",
                false,
                "2026-11-15T11:00:00+08:00",
            ),
            (
                "remove-image",
                "image",
                "Image 2 x 2",
                false,
                "2026-11-15T10:00:00+08:00",
            ),
        ],
    );
    let asset_path = data.join("assets/remove-image.png");
    RgbaImage::from_pixel(2, 2, Rgba([52, 211, 153, 255]))
        .save(&asset_path)
        .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'remove-image'",
            rusqlite::params![asset_path.to_string_lossy()],
        )
        .unwrap();
    drop(connection);
    let service = ClipboardService::open(&data).unwrap();

    let pinned = service
        .request(
            "clipboard.history.pin",
            json!({ "id": "pin-now", "isPinned": true }),
        )
        .await
        .unwrap();
    assert_eq!(pinned["id"], "pin-now");
    assert_eq!(pinned["isPinned"], true);

    let cleared = service
        .request("clipboard.history.clearUnpinned", json!({}))
        .await
        .unwrap();
    assert_eq!(cleared, json!({ "deletedCount": 1 }));
    assert!(!asset_path.exists());
    let remaining = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert_eq!(item_ids(&remaining), ["keep-pinned", "pin-now"]);

    let deleted = service
        .request("clipboard.history.delete", json!({ "id": "pin-now" }))
        .await
        .unwrap();
    assert_eq!(deleted, json!({ "deleted": true }));

    for method in ["clipboard.history.pin", "clipboard.history.delete"] {
        let params = if method.ends_with("pin") {
            json!({ "id": "missing", "isPinned": true })
        } else {
            json!({ "id": "missing" })
        };
        let error = service.request(method, params).await.unwrap_err();
        assert_eq!(error.code, "CLIPBOARD_SERVICE_ITEM_NOT_FOUND");
    }
}

#[tokio::test]
async fn restore_uses_original_content_and_touches_only_after_backend_success() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_history(
        &data,
        &[
            (
                "restore-text",
                "text",
                "original text",
                false,
                "2026-11-15T12:00:00+08:00",
            ),
            (
                "restore-files",
                "files",
                "ignored display text",
                false,
                "2026-11-15T11:00:00+08:00",
            ),
            (
                "restore-image",
                "image",
                "Image 2 x 3",
                false,
                "2026-11-15T10:00:00+08:00",
            ),
        ],
    );
    let asset_path = data.join("assets/restore-image.png");
    RgbaImage::from_pixel(2, 3, Rgba([12, 34, 56, 255]))
        .save(&asset_path)
        .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET file_paths_json = ?1 WHERE id = 'restore-files'",
            rusqlite::params![r#"["/tmp/a.txt","/tmp/b.txt"]"#],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'restore-image'",
            rusqlite::params![asset_path.to_string_lossy()],
        )
        .unwrap();
    drop(connection);
    let backend = Arc::new(RecordingClipboardBackend::default());
    let service = ClipboardService::open_with_backend(&data, backend.clone()).unwrap();

    for id in ["restore-text", "restore-files", "restore-image"] {
        let restored = service
            .request("clipboard.history.restore", json!({ "id": id }))
            .await
            .unwrap();
        assert_eq!(restored["restored"], true);
        assert!(restored["lastCopiedAt"].as_str().is_some());
    }
    assert_eq!(
        *backend.writes.lock().unwrap(),
        [
            "text:original text",
            "text:/tmp/a.txt\n/tmp/b.txt",
            "image:2x3",
        ]
    );

    backend.fail_next.store(true, Ordering::Release);
    let before = service
        .request("clipboard.history.get", json!({ "id": "restore-text" }))
        .await
        .unwrap()["lastCopiedAt"]
        .clone();
    let error = service
        .request("clipboard.history.restore", json!({ "id": "restore-text" }))
        .await
        .unwrap_err();
    assert_eq!(error.code, "CLIPBOARD_SERVICE_CLIPBOARD_WRITE_FAILED");
    assert_eq!(error.message, "cannot write clipboard history item");
    let after = service
        .request("clipboard.history.get", json!({ "id": "restore-text" }))
        .await
        .unwrap()["lastCopiedAt"]
        .clone();
    assert_eq!(after, before);
}

#[tokio::test]
async fn settings_update_registers_before_commit_rolls_back_on_failure_and_enforces_retention() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_history(&data, &[]);
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    for index in 0..52 {
        connection
            .execute(
                r#"
                INSERT INTO clipboard_items (
                    id, kind, preview, content_text, asset_path, file_paths_json,
                    content_hash, is_pinned, created_at, last_copied_at
                ) VALUES (?1, 'text', ?1, ?1, NULL, '[]', ?2, ?3, ?4, NULL)
                "#,
                rusqlite::params![
                    format!("item-{index:02}"),
                    format!("hash-{index:02}"),
                    i64::from(index == 0),
                    format!("2026-11-15T10:{index:02}:00+08:00"),
                ],
            )
            .unwrap();
    }
    drop(connection);
    let backend = Arc::new(RecordingClipboardBackend::with_shortcut(
        "CommandOrControl+Alt+V",
    ));
    let service = ClipboardService::open_with_backend(&data, backend.clone()).unwrap();

    let initial = service
        .request("clipboard.settings.get", json!({}))
        .await
        .unwrap();
    assert_eq!(initial["captureEnabled"], true);
    assert_eq!(initial["retentionLimit"], 500);
    assert_eq!(initial["shortcut"], "CommandOrControl+Alt+V");
    assert_eq!(initial["shortcutRegistered"], true);

    let updated = service
        .request(
            "clipboard.settings.update",
            json!({
                "settings": {
                    "captureEnabled": false,
                    "retentionLimit": 50,
                    "shortcut": "CommandOrControl+Shift+X"
                }
            }),
        )
        .await
        .unwrap();
    assert_eq!(updated["captureEnabled"], false);
    assert_eq!(updated["retentionLimit"], 50);
    assert_eq!(updated["shortcut"], "CommandOrControl+Shift+X");
    assert_eq!(
        backend.shortcut.lock().unwrap().as_str(),
        "CommandOrControl+Shift+X"
    );
    let retained = service
        .request("clipboard.history.list", json!({ "limit": 100 }))
        .await
        .unwrap();
    assert_eq!(retained["items"].as_array().unwrap().len(), 50);
    assert!(item_ids(&retained).contains(&"item-00"));

    backend.fail_shortcut.store(true, Ordering::Release);
    let error = service
        .request(
            "clipboard.settings.update",
            json!({
                "settings": {
                    "captureEnabled": true,
                    "retentionLimit": 200,
                    "shortcut": "CommandOrControl+Shift+Y"
                }
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "CLIPBOARD_SERVICE_SHORTCUT_REGISTER_FAILED");
    let unchanged = service
        .request("clipboard.settings.get", json!({}))
        .await
        .unwrap();
    assert_eq!(unchanged["captureEnabled"], false);
    assert_eq!(unchanged["retentionLimit"], 50);
    assert_eq!(unchanged["shortcut"], "CommandOrControl+Shift+X");
    assert_eq!(
        backend.shortcut.lock().unwrap().as_str(),
        "CommandOrControl+Shift+X"
    );

    let error = service
        .request(
            "clipboard.settings.update",
            json!({
                "settings": {
                    "captureEnabled": true,
                    "retentionLimit": 49,
                    "shortcut": "CommandOrControl+Shift+X"
                }
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "CLIPBOARD_SERVICE_INVALID_PARAMS");
}

#[tokio::test]
async fn methods_reject_unapproved_params_and_invalid_cursor_context() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    let long_query = "shared".repeat(2_000);
    let first_content = format!("{long_query} first");
    let second_content = format!("{long_query} second");
    seed_history(
        &data,
        &[
            (
                "first",
                "text",
                &first_content,
                false,
                "2026-11-15T11:00:00+08:00",
            ),
            (
                "second",
                "text",
                &second_content,
                false,
                "2026-11-15T10:00:00+08:00",
            ),
        ],
    );
    let service = ClipboardService::open(&data).unwrap();

    for params in [json!({ "limit": 0 }), json!({ "limit": 101 })] {
        let error = service
            .request("clipboard.history.list", params)
            .await
            .unwrap_err();
        assert_eq!(error.code, "CLIPBOARD_SERVICE_INVALID_PARAMS");
    }
    let first = service
        .request(
            "clipboard.history.list",
            json!({ "query": long_query, "limit": 1 }),
        )
        .await
        .unwrap();
    let cursor = first["nextCursor"].as_str().unwrap();
    assert!(cursor.len() < 512);
    let error = service
        .request(
            "clipboard.history.list",
            json!({ "query": "second", "cursor": cursor }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "CLIPBOARD_SERVICE_INVALID_PARAMS");

    let cases = [
        (
            "clipboard.history.get",
            json!({ "id": "first", "assetPath": "/private" }),
        ),
        (
            "clipboard.history.clearUnpinned",
            json!({ "includePinned": true }),
        ),
        (
            "clipboard.settings.update",
            json!({
                "settings": {
                    "captureEnabled": true,
                    "retentionLimit": 500,
                    "shortcut": "CommandOrControl+Alt+V",
                    "databasePath": "/private"
                }
            }),
        ),
    ];
    for (method, params) in cases {
        let error = service.request(method, params).await.unwrap_err();
        assert_eq!(error.code, "CLIPBOARD_SERVICE_INVALID_PARAMS");
    }
}

#[tokio::test]
async fn list_defaults_to_fifty_and_pages_at_the_hundred_item_maximum() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_history(&data, &[]);
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    for index in 0..101 {
        connection
            .execute(
                r#"
                INSERT INTO clipboard_items (
                    id, kind, preview, content_text, asset_path, file_paths_json,
                    content_hash, is_pinned, created_at, last_copied_at
                ) VALUES (?1, 'text', ?1, ?1, NULL, '[]', ?2, 0, ?3, NULL)
                "#,
                rusqlite::params![
                    format!("item-{index:03}"),
                    format!("hash-{index:03}"),
                    format!("2026-11-15T{index:03}:00:00+08:00"),
                ],
            )
            .unwrap();
    }
    drop(connection);
    let service = ClipboardService::open(&data).unwrap();

    let default_page = service
        .request("clipboard.history.list", json!({}))
        .await
        .unwrap();
    assert_eq!(default_page["items"].as_array().unwrap().len(), 50);
    assert!(default_page["nextCursor"].as_str().is_some());

    let first = service
        .request("clipboard.history.list", json!({ "limit": 100 }))
        .await
        .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 100);
    let first_ids = item_ids(&first);
    let cursor = first["nextCursor"].as_str().unwrap();
    let second = service
        .request(
            "clipboard.history.list",
            json!({ "limit": 100, "cursor": cursor }),
        )
        .await
        .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(!first_ids.contains(&item_ids(&second)[0]));
    assert!(second["nextCursor"].is_null());
}

#[tokio::test]
async fn delete_refuses_tampered_image_paths_without_deleting_external_files_or_rows() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_history(
        &data,
        &[(
            "image",
            "image",
            "Image 1 x 1",
            false,
            "2026-11-15T10:00:00+08:00",
        )],
    );
    let asset_path = data.join("assets/image.png");
    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))
        .save(&asset_path)
        .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'image'",
            rusqlite::params![asset_path.to_string_lossy()],
        )
        .unwrap();
    drop(connection);
    let service = ClipboardService::open(&data).unwrap();
    let external = temporary.path().join("external.png");
    RgbaImage::from_pixel(1, 1, Rgba([4, 5, 6, 255]))
        .save(&external)
        .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'image'",
            rusqlite::params![external.to_string_lossy()],
        )
        .unwrap();
    drop(connection);

    let error = service
        .request("clipboard.history.delete", json!({ "id": "image" }))
        .await
        .unwrap_err();
    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");
    assert!(!error.message.contains(external.to_string_lossy().as_ref()));
    assert!(external.exists());

    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'image'",
            rusqlite::params![asset_path.to_string_lossy()],
        )
        .unwrap();
    drop(connection);
    service
        .request("clipboard.history.delete", json!({ "id": "image" }))
        .await
        .unwrap();
    assert!(!asset_path.exists());
}

#[tokio::test]
async fn pin_does_not_mutate_history_when_image_response_cannot_be_built() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_history(
        &data,
        &[(
            "image",
            "image",
            "Image 1 x 1",
            false,
            "2026-11-15T10:00:00+08:00",
        )],
    );
    let asset_path = data.join("assets/image.png");
    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))
        .save(&asset_path)
        .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'image'",
            rusqlite::params![asset_path.to_string_lossy()],
        )
        .unwrap();
    drop(connection);
    let service = ClipboardService::open(&data).unwrap();
    fs::write(&asset_path, b"corrupt after startup").unwrap();

    let error = service
        .request(
            "clipboard.history.pin",
            json!({ "id": "image", "isPinned": true }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");

    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))
        .save(&asset_path)
        .unwrap();
    let item = service
        .request("clipboard.history.get", json!({ "id": "image" }))
        .await
        .unwrap();
    assert_eq!(item["isPinned"], false);
}

#[tokio::test]
async fn clear_preflights_all_assets_before_deleting_any_history() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_history(
        &data,
        &[
            (
                "text",
                "text",
                "must survive failed clear",
                false,
                "2026-11-15T11:00:00+08:00",
            ),
            (
                "image",
                "image",
                "Image 1 x 1",
                false,
                "2026-11-15T10:00:00+08:00",
            ),
        ],
    );
    let asset_path = data.join("assets/image.png");
    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))
        .save(&asset_path)
        .unwrap();
    let connection = Connection::open(data.join("clipboard_history.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET asset_path = ?1 WHERE id = 'image'",
            rusqlite::params![asset_path.to_string_lossy()],
        )
        .unwrap();
    drop(connection);
    let service = ClipboardService::open(&data).unwrap();
    fs::remove_file(&asset_path).unwrap();

    let error = service
        .request("clipboard.history.clearUnpinned", json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "CLIPBOARD_SERVICE_DATA_INVALID");

    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))
        .save(&asset_path)
        .unwrap();
    let remaining = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert_eq!(item_ids(&remaining), ["text", "image"]);
}

#[derive(Default)]
struct RecordingClipboardBackend {
    writes: Mutex<Vec<String>>,
    fail_next: AtomicBool,
    shortcut: Mutex<String>,
    fail_shortcut: AtomicBool,
}

impl ClipboardBackend for RecordingClipboardBackend {
    fn write_text(&self, value: &str) -> Result<(), String> {
        self.record(format!("text:{value}"))
    }

    fn write_image(&self, value: &image::DynamicImage) -> Result<(), String> {
        self.record(format!("image:{}x{}", value.width(), value.height()))
    }

    fn shortcut_status(&self) -> ClipboardShortcutStatus {
        ClipboardShortcutStatus {
            registered: !self.shortcut.lock().unwrap().is_empty(),
            register_error: None,
            unavailable_reason: None,
            system_shortcut_command: None,
        }
    }

    fn replace_shortcut(&self, _previous: &str, next: &str) -> Result<(), String> {
        if self.fail_shortcut.swap(false, Ordering::AcqRel) {
            return Err("shortcut conflict at /private/path".into());
        }
        *self.shortcut.lock().unwrap() = next.to_string();
        Ok(())
    }
}

impl RecordingClipboardBackend {
    fn with_shortcut(shortcut: &str) -> Self {
        Self {
            shortcut: Mutex::new(shortcut.to_string()),
            ..Self::default()
        }
    }

    fn record(&self, value: String) -> Result<(), String> {
        if self.fail_next.swap(false, Ordering::AcqRel) {
            return Err("private backend failure at /private/path".into());
        }
        self.writes.lock().unwrap().push(value);
        Ok(())
    }
}

#[test]
fn shortcut_activation_broadcasts_only_the_signed_history_window_request() {
    let temporary = tempfile::tempdir().unwrap();
    let service = ClipboardService::open(temporary.path()).unwrap();
    let mut events = service.subscribe();

    service.open_history();

    let event = events.try_recv().unwrap();
    assert_eq!(
        event,
        ClipboardServiceEvent::HostUiOpen {
            window_id: "history".into(),
            payload: json!({}),
        }
    );
    let serialized = serde_json::to_string(&event).unwrap();
    assert!(!serialized.contains("content"));
    assert!(!serialized.contains("path"));
}

fn item_ids(result: &Value) -> Vec<&str> {
    result["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect()
}

fn seed_history(data: &std::path::Path, items: &[(&str, &str, &str, bool, &str)]) {
    fs::create_dir_all(data.join("assets")).unwrap();
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
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
            INSERT INTO clipboard_settings VALUES ('capture_enabled', '1');
            INSERT INTO clipboard_settings VALUES ('retention_limit', '500');
            INSERT INTO clipboard_settings VALUES ('shortcut', 'CommandOrControl+Alt+V');
            "#,
        )
        .unwrap();
    for (id, kind, content, is_pinned, created_at) in items {
        connection
            .execute(
                r#"
                INSERT INTO clipboard_items (
                    id, kind, preview, content_text, asset_path, file_paths_json,
                    content_hash, is_pinned, created_at, last_copied_at
                ) VALUES (?1, ?2, ?3, ?3, NULL, '[]', ?4, ?5, ?6, NULL)
                "#,
                rusqlite::params![
                    id,
                    kind,
                    content,
                    format!("hash-{id}"),
                    i64::from(*is_pinned),
                    created_at
                ],
            )
            .unwrap();
    }
}
