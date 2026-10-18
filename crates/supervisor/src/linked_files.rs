//! Read-only host file previews. Besides the owner, only a whole-device grant with
//! workspace write access reaches them (`access::linked_files_allowed`).
use crate::http::{static_content_type, stream_file, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Deserialize)]
pub(crate) struct FileQuery {
    path: String,
    offset: Option<u64>,
    limit: Option<usize>,
}

pub(crate) async fn read(
    Path((id, action)): Path<(String, String)>,
    Query(query): Query<FileQuery>,
    State(state): State<AppState>,
) -> Response {
    async fn handle(
        state: AppState,
        id: String,
        action: String,
        query: FileQuery,
    ) -> anyhow::Result<Response> {
        state.get_thread(&id)?;
        anyhow::ensure!(
            matches!(action.as_str(), "stat" | "preview" | "raw"),
            "Unknown file action"
        );
        let path = std::path::PathBuf::from(&query.path);
        anyhow::ensure!(path.is_absolute(), "Linked file path must be absolute");
        let mut file = tokio::fs::File::open(&path).await?;
        let metadata = file.metadata().await?;
        anyhow::ensure!(
            metadata.is_file(),
            "Linked path must point to a regular file"
        );
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if action == "stat" {
            return Ok(Json(
                json!({"path":query.path,"name":name,"kind":"file","size":metadata.len()}),
            )
            .into_response());
        }
        if action == "preview" {
            let offset = query.offset.unwrap_or(0).min(metadata.len());
            let limit = query.limit.unwrap_or(24_000).clamp(1, 100_000);
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            let mut bytes = Vec::new();
            file.take(limit as u64).read_to_end(&mut bytes).await?;
            let next = offset + bytes.len() as u64;
            return Ok(Json(json!({"path":query.path,"name":name,"content":String::from_utf8_lossy(&bytes),"language":pockymoe_runtime::files::language_for(&name),"size":metadata.len(),"truncated":next<metadata.len(),"nextOffset":next})).into_response());
        }
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, static_content_type(&path))
            .header(header::CONTENT_LENGTH, metadata.len())
            .header(header::CACHE_CONTROL, "private, no-store")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(
                "content-security-policy",
                "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data: blob:",
            )
            .header("referrer-policy", "no-referrer")
            .body(stream_file(file, ()))?)
    }
    match handle(state, id, action, query).await {
        Ok(response) => response,
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({"code":"file_unavailable","message":format!("Cannot preview linked file: {error}")}))).into_response(),
    }
}
