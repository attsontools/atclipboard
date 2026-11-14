use std::{path::Path, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{ClipboardItemKind, ClipboardStore, ServiceError};

#[derive(Clone)]
pub struct ClipboardService {
    store: Arc<ClipboardStore>,
}

impl ClipboardService {
    pub fn open(data_directory: &Path) -> Result<Self, ServiceError> {
        let (store, _) = ClipboardStore::open(data_directory)?;
        Ok(Self {
            store: Arc::new(store),
        })
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
                let items = self
                    .store
                    .list_items(params.kind, params.query.as_deref(), limit)?
                    .into_iter()
                    .map(PublicClipboardItem::from)
                    .collect::<Vec<_>>();
                Ok(json!({ "items": items, "nextCursor": Value::Null }))
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
    limit: Option<usize>,
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

impl From<crate::ClipboardHistoryItem> for PublicClipboardItem {
    fn from(item: crate::ClipboardHistoryItem) -> Self {
        Self {
            id: item.id,
            kind: item.kind,
            preview: item.preview,
            content_text: item.content_text,
            content_truncated: false,
            file_paths: item.file_paths,
            preview_data_url: None,
            is_pinned: item.is_pinned,
            created_at: item.created_at,
            last_copied_at: item.last_copied_at,
        }
    }
}

fn decode_params<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, ServiceError> {
    serde_json::from_value(params)
        .map_err(|_| invalid_params("clipboard service params are invalid"))
}

fn invalid_params(message: impl Into<String>) -> ServiceError {
    ServiceError::new("CLIPBOARD_SERVICE_INVALID_PARAMS", message)
}
