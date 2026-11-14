use std::fs;

use atclipboard_service::{ServiceClient, ServiceServer};
use rusqlite::Connection;
use serde_json::json;

#[tokio::test]
async fn real_transport_exposes_the_bounded_eight_method_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_text_history(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    let endpoint = data.join("service.endpoint.json");
    let server = ServiceServer::bind(endpoint.clone(), data.clone(), "0.1.0-alpha.0")
        .await
        .unwrap();
    let stop = server.stop_handle();
    let task = tokio::spawn(server.run());
    let client = ServiceClient::connect(&endpoint, &data, "transport-test")
        .await
        .unwrap();

    let result = client
        .request("clipboard.history.list", json!({ "limit": 20 }))
        .await
        .unwrap();

    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(result["items"][0]["id"], "clip-text");
    assert_eq!(result["items"][0]["contentText"], "hello clipboard");
    assert!(result["nextCursor"].is_null());
    assert!(!result.to_string().contains("assetPath"));

    let pinned = client
        .request(
            "clipboard.history.pin",
            json!({ "id": "clip-text", "isPinned": true }),
        )
        .await
        .unwrap();
    assert_eq!(pinned["isPinned"], true);
    let item = client
        .request("clipboard.history.get", json!({ "id": "clip-text" }))
        .await
        .unwrap();
    assert_eq!(item["contentText"], "hello clipboard");

    let settings = client
        .request("clipboard.settings.get", json!({}))
        .await
        .unwrap();
    assert_eq!(settings["retentionLimit"], 500);
    let settings = client
        .request(
            "clipboard.settings.update",
            json!({
                "settings": {
                    "captureEnabled": false,
                    "retentionLimit": 50,
                    "shortcut": "CommandOrControl+Alt+V"
                }
            }),
        )
        .await
        .unwrap();
    assert_eq!(settings["captureEnabled"], false);

    let restore_error = client
        .request("clipboard.history.restore", json!({ "id": "clip-text" }))
        .await
        .unwrap_err();
    assert_eq!(
        restore_error.code,
        "CLIPBOARD_SERVICE_CLIPBOARD_WRITE_FAILED"
    );
    let cleared = client
        .request("clipboard.history.clearUnpinned", json!({}))
        .await
        .unwrap();
    assert_eq!(cleared["deletedCount"], 0);
    let deleted = client
        .request("clipboard.history.delete", json!({ "id": "clip-text" }))
        .await
        .unwrap();
    assert_eq!(deleted["deleted"], true);
    let missing = client
        .request("clipboard.history.get", json!({ "id": "clip-text" }))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "CLIPBOARD_SERVICE_ITEM_NOT_FOUND");

    drop(client);
    stop.notify_one();
    task.await.unwrap().unwrap();
    assert!(!endpoint.exists());
    assert!(!data.join("service.lock").exists());
}

fn seed_text_history(data: &std::path::Path) {
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
}
