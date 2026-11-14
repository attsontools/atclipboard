use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use atclipboard_service::{ClipboardBackend, ClipboardServiceEvent, ServiceClient, ServiceServer};
use rusqlite::Connection;
use serde_json::json;

#[tokio::test]
async fn real_transport_exposes_the_bounded_eight_method_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    seed_text_history(&data);
    fs::write(data.join("clipboard-takeover-capability-v1"), b"1\n").unwrap();
    let endpoint = data.join("service.endpoint.json");
    let server = ServiceServer::bind_with_backend(
        endpoint.clone(),
        data.clone(),
        "0.1.0-alpha.0",
        Arc::new(UnavailableTestBackend),
    )
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

#[tokio::test]
async fn all_clients_receive_content_free_history_events_with_monotonic_revisions() {
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
    let first = ServiceClient::connect(&endpoint, &data, "first-client")
        .await
        .unwrap();
    let second = ServiceClient::connect(&endpoint, &data, "second-client")
        .await
        .unwrap();
    let mut first_events = first.subscribe();
    let mut second_events = second.subscribe();

    first
        .request(
            "clipboard.history.pin",
            json!({ "id": "clip-text", "isPinned": true }),
        )
        .await
        .unwrap();

    for receiver in [&mut first_events, &mut second_events] {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event, ClipboardServiceEvent::HistoryChanged { revision: 1 });
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains("content"));
        assert!(!serialized.contains("path"));
    }

    second
        .request(
            "clipboard.history.pin",
            json!({ "id": "clip-text", "isPinned": false }),
        )
        .await
        .unwrap();
    for receiver in [&mut first_events, &mut second_events] {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event, ClipboardServiceEvent::HistoryChanged { revision: 2 });
    }

    drop(first);
    drop(second);
    stop.notify_one();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn one_background_watcher_serves_all_connected_clients() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    let endpoint = data.join("service.endpoint.json");
    let backend = Arc::new(BackgroundTextBackend {
        reads: AtomicUsize::new(0),
    });
    let server = ServiceServer::bind_with_backend(
        endpoint.clone(),
        data.clone(),
        "0.1.0-alpha.0",
        backend.clone(),
    )
    .await
    .unwrap();
    let stop = server.stop_handle();
    let task = tokio::spawn(server.run());
    let first = ServiceClient::connect(&endpoint, &data, "first-client")
        .await
        .unwrap();
    let second = ServiceClient::connect(&endpoint, &data, "second-client")
        .await
        .unwrap();
    let mut first_events = first.subscribe();
    let mut second_events = second.subscribe();

    let first_event = tokio::time::timeout(std::time::Duration::from_secs(2), first_events.recv())
        .await
        .unwrap()
        .unwrap();
    let second_event =
        tokio::time::timeout(std::time::Duration::from_secs(1), second_events.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(first_event, second_event);
    assert_eq!(
        first_event,
        ClipboardServiceEvent::HistoryChanged { revision: 1 }
    );
    assert!(backend.reads.load(Ordering::Acquire) >= 1);
    let history = first
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["items"][0]["contentText"], "background text");

    drop(first);
    drop(second);
    stop.notify_one();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn event_subscribers_close_when_the_service_connection_ends() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    let endpoint = data.join("service.endpoint.json");
    let server = ServiceServer::bind_with_backend(
        endpoint.clone(),
        data.clone(),
        "0.1.0-alpha.0",
        Arc::new(UnavailableTestBackend),
    )
    .await
    .unwrap();
    let stop = server.stop_handle();
    let task = tokio::spawn(server.run());
    let client = ServiceClient::connect(&endpoint, &data, "event-close-client")
        .await
        .unwrap();
    let mut events = client.subscribe();

    stop.notify_one();
    task.await.unwrap().unwrap();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .expect("event receiver should close promptly");
    assert_eq!(
        result,
        Err(tokio::sync::broadcast::error::RecvError::Closed)
    );
}

struct BackgroundTextBackend {
    reads: AtomicUsize,
}

struct UnavailableTestBackend;

impl ClipboardBackend for UnavailableTestBackend {
    fn write_text(&self, _value: &str) -> Result<(), String> {
        Err("clipboard unavailable".into())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Err("clipboard unavailable".into())
    }
}

impl ClipboardBackend for BackgroundTextBackend {
    fn read_text(&self) -> Result<Option<String>, String> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        Ok(Some("background text".into()))
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }
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
