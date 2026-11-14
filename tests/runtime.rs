use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Barrier,
};

use atclipboard_service::{
    ClipboardBackend, ClipboardImage, ClipboardRuntime, ClipboardService, ClipboardServiceEvent,
    ClipboardShortcutStatus,
};
use serde_json::json;

struct FakeClipboardBackend;

impl ClipboardBackend for FakeClipboardBackend {
    fn read_files(&self) -> Result<Option<Vec<String>>, String> {
        Ok(Some(vec!["/tmp/report.csv".into()]))
    }

    fn read_text(&self) -> Result<Option<String>, String> {
        Ok(Some("text that must not shadow copied files".into()))
    }

    fn read_image(&self) -> Result<Option<ClipboardImage>, String> {
        Ok(None)
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }

    fn shortcut_status(&self) -> ClipboardShortcutStatus {
        ClipboardShortcutStatus {
            registered: true,
            register_error: None,
            unavailable_reason: None,
            system_shortcut_command: None,
        }
    }
}

struct TextAndImageBackend;

struct TextFilePathsBackend;

struct StaticTextBackend;

impl ClipboardBackend for StaticTextBackend {
    fn read_text(&self) -> Result<Option<String>, String> {
        Ok(Some("clipboard text".into()))
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }
}

impl ClipboardBackend for TextFilePathsBackend {
    fn read_text(&self) -> Result<Option<String>, String> {
        Ok(Some("file:///tmp/report.csv\n/tmp/image.png".into()))
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }
}

impl ClipboardBackend for TextAndImageBackend {
    fn read_files(&self) -> Result<Option<Vec<String>>, String> {
        Ok(None)
    }

    fn read_text(&self) -> Result<Option<String>, String> {
        Ok(Some("captured text".into()))
    }

    fn read_image(&self) -> Result<Option<ClipboardImage>, String> {
        Ok(Some(ClipboardImage {
            rgba: vec![12, 34, 56, 255, 78, 90, 12, 255],
            width: 2,
            height: 1,
        }))
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }
}

struct CountingTextBackend {
    reads: AtomicUsize,
}

struct BarrierTextBackend {
    barrier: Barrier,
}

impl ClipboardBackend for BarrierTextBackend {
    fn read_text(&self) -> Result<Option<String>, String> {
        self.barrier.wait();
        Ok(Some("racing clipboard content".into()))
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }
}

struct FailingReadBackend;

struct TextThenImageReadFailureBackend;

impl ClipboardBackend for TextThenImageReadFailureBackend {
    fn read_text(&self) -> Result<Option<String>, String> {
        Ok(Some("must not be partially committed".into()))
    }

    fn read_image(&self) -> Result<Option<ClipboardImage>, String> {
        Err("image clipboard failed".into())
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }
}

impl ClipboardBackend for FailingReadBackend {
    fn read_files(&self) -> Result<Option<Vec<String>>, String> {
        Err("backend failure at /private/clipboard.sock".into())
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }
}

impl CountingTextBackend {
    fn new() -> Self {
        Self {
            reads: AtomicUsize::new(0),
        }
    }
}

impl ClipboardBackend for CountingTextBackend {
    fn read_text(&self) -> Result<Option<String>, String> {
        let index = self.reads.fetch_add(1, Ordering::AcqRel);
        Ok(Some(format!("captured text {index}")))
    }

    fn write_text(&self, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn write_image(&self, _value: &image::DynamicImage) -> Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
async fn runtime_tick_records_copied_files_before_text_from_the_same_clipboard_state() {
    let temporary = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeClipboardBackend);
    let service = ClipboardService::open_with_backend(temporary.path(), backend).unwrap();
    let runtime = ClipboardRuntime::new(service.clone());

    let outcome = runtime.tick().unwrap();

    assert_eq!(outcome.inserted, 1);
    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["items"][0]["kind"], "files");
    assert_eq!(history["items"][0]["filePaths"], json!(["/tmp/report.csv"]));
}

#[tokio::test]
async fn runtime_tick_preserves_legacy_absolute_file_paths_from_text() {
    let temporary = tempfile::tempdir().unwrap();
    let service =
        ClipboardService::open_with_backend(temporary.path(), Arc::new(TextFilePathsBackend))
            .unwrap();

    let outcome = ClipboardRuntime::new(service.clone()).tick().unwrap();

    assert_eq!(outcome.inserted, 1);
    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert_eq!(history["items"][0]["kind"], "files");
    assert_eq!(
        history["items"][0]["filePaths"],
        json!(["/tmp/report.csv", "/tmp/image.png"])
    );
}

#[tokio::test]
async fn runtime_tick_records_text_and_image_from_the_same_clipboard_state() {
    let temporary = tempfile::tempdir().unwrap();
    let backend = Arc::new(TextAndImageBackend);
    let service = ClipboardService::open_with_backend(temporary.path(), backend).unwrap();
    let runtime = ClipboardRuntime::new(service.clone());

    let outcome = runtime.tick().unwrap();

    assert_eq!(outcome.inserted, 2);
    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    let items = history["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .any(|item| item["kind"] == "text" && item["contentText"] == "captured text"));
    assert!(items.iter().any(|item| {
        item["kind"] == "image"
            && item["previewDataUrl"]
                .as_str()
                .is_some_and(|value| value.starts_with("data:image/png;base64,"))
    }));
}

#[tokio::test]
async fn repeated_runtime_ticks_do_not_duplicate_an_unchanged_clipboard_state() {
    let temporary = tempfile::tempdir().unwrap();
    let service =
        ClipboardService::open_with_backend(temporary.path(), Arc::new(TextAndImageBackend))
            .unwrap();
    let runtime = ClipboardRuntime::new(service.clone());

    assert_eq!(runtime.tick().unwrap().inserted, 2);
    assert_eq!(runtime.tick().unwrap().inserted, 0);

    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        temporary.path().join("assets").read_dir().unwrap().count(),
        1
    );
}

#[tokio::test]
async fn deleting_the_current_text_does_not_reinsert_it_until_the_clipboard_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let service =
        ClipboardService::open_with_backend(temporary.path(), Arc::new(StaticTextBackend)).unwrap();
    let runtime = ClipboardRuntime::new(service.clone());
    assert_eq!(runtime.tick().unwrap().inserted, 1);
    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    let id = history["items"][0]["id"].as_str().unwrap();
    service
        .request("clipboard.history.delete", json!({ "id": id }))
        .await
        .unwrap();

    assert_eq!(runtime.tick().unwrap().inserted, 0);
    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert!(history["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn disabled_capture_skips_clipboard_reads_and_history_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let backend = Arc::new(CountingTextBackend::new());
    let service = ClipboardService::open_with_backend(temporary.path(), backend.clone()).unwrap();
    service
        .request(
            "clipboard.settings.update",
            json!({
                "settings": {
                    "captureEnabled": false,
                    "retentionLimit": 500,
                    "shortcut": "CommandOrControl+Alt+V"
                }
            }),
        )
        .await
        .unwrap();

    let outcome = ClipboardRuntime::new(service.clone()).tick().unwrap();

    assert_eq!(outcome.inserted, 0);
    assert_eq!(backend.reads.load(Ordering::Acquire), 0);
    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert!(history["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn each_insert_enforces_the_current_retention_limit() {
    let temporary = tempfile::tempdir().unwrap();
    let service =
        ClipboardService::open_with_backend(temporary.path(), Arc::new(CountingTextBackend::new()))
            .unwrap();
    let runtime = ClipboardRuntime::new(service.clone());
    for _ in 0..50 {
        assert_eq!(runtime.tick().unwrap().inserted, 1);
    }
    service
        .request(
            "clipboard.settings.update",
            json!({
                "settings": {
                    "captureEnabled": true,
                    "retentionLimit": 50,
                    "shortcut": "CommandOrControl+Alt+V"
                }
            }),
        )
        .await
        .unwrap();

    let outcome = runtime.tick().unwrap();

    assert_eq!(outcome.inserted, 1);
    assert_eq!(outcome.deleted_by_retention, 1);
    let history = service
        .request("clipboard.history.list", json!({ "limit": 100 }))
        .await
        .unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 50);
}

#[test]
fn a_runtime_tick_broadcasts_one_content_free_change_event_for_its_batch() {
    let temporary = tempfile::tempdir().unwrap();
    let service =
        ClipboardService::open_with_backend(temporary.path(), Arc::new(TextAndImageBackend))
            .unwrap();
    let mut events = service.subscribe();

    let outcome = ClipboardRuntime::new(service).tick().unwrap();

    assert_eq!(outcome.inserted, 2);
    assert_eq!(
        events.try_recv().unwrap(),
        ClipboardServiceEvent::HistoryChanged { revision: 1 }
    );
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn concurrent_ticks_for_the_same_clipboard_state_insert_once() {
    let temporary = tempfile::tempdir().unwrap();
    let service = ClipboardService::open_with_backend(
        temporary.path(),
        Arc::new(BarrierTextBackend {
            barrier: Barrier::new(2),
        }),
    )
    .unwrap();
    let runtime = ClipboardRuntime::new(service.clone());
    let first = std::thread::spawn({
        let runtime = runtime.clone();
        move || runtime.tick().unwrap()
    });
    let second = std::thread::spawn(move || runtime.tick().unwrap());

    let inserted = first.join().unwrap().inserted + second.join().unwrap().inserted;

    assert_eq!(inserted, 1);
    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
}

#[test]
fn backend_read_errors_use_a_stable_path_free_error() {
    let temporary = tempfile::tempdir().unwrap();
    let service =
        ClipboardService::open_with_backend(temporary.path(), Arc::new(FailingReadBackend))
            .unwrap();

    let error = ClipboardRuntime::new(service).tick().unwrap_err();

    assert_eq!(error.code, "CLIPBOARD_SERVICE_CLIPBOARD_READ_FAILED");
    assert_eq!(error.message, "cannot read the system clipboard");
    assert!(!error.message.contains('/'));
}

#[tokio::test]
async fn a_later_clipboard_read_failure_does_not_commit_a_partial_tick() {
    let temporary = tempfile::tempdir().unwrap();
    let service = ClipboardService::open_with_backend(
        temporary.path(),
        Arc::new(TextThenImageReadFailureBackend),
    )
    .unwrap();

    let error = ClipboardRuntime::new(service.clone()).tick().unwrap_err();

    assert_eq!(error.code, "CLIPBOARD_SERVICE_CLIPBOARD_READ_FAILED");
    let history = service
        .request("clipboard.history.list", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert!(history["items"].as_array().unwrap().is_empty());
}
