use axum::{extract::{State, DefaultBodyLimit}, http::StatusCode, routing::get, Json, Router};
use editor_core::{Chapter, SaveChapter};
use std::{sync::Arc, path::PathBuf};
use tokio::sync::Mutex;
use tower_http::{cors::CorsLayer, services::ServeDir};

#[derive(Clone)]
struct AppState { chapter: Arc<Mutex<Chapter>>, file: PathBuf }

async fn read(State(state): State<AppState>) -> Json<Chapter> {
    Json(state.chapter.lock().await.clone())
}
async fn save(State(state): State<AppState>, Json(input): Json<SaveChapter>) -> Result<Json<Chapter>, (StatusCode, String)> {
    let mut doc = editor_core::validate(&input).map_err(|s| (StatusCode::BAD_REQUEST,s.to_string()))?;
    let mut current = state.chapter.lock().await;
    if current.revision != input.expected_revision { return Err((StatusCode::CONFLICT, "Глава была изменена в другой вкладке".into())); }
    if current.content_hash == doc.content_hash && current.title == doc.title { return Ok(Json(current.clone())); }
    doc.revision = current.revision + 1;
    let json = serde_json::to_vec_pretty(&doc).map_err(internal)?;
    let temp = state.file.with_extension("json.tmp");
    tokio::fs::write(&temp, json).await.map_err(internal)?;
    tokio::fs::rename(temp, &state.file).await.map_err(internal)?;
    *current = doc.clone();
    Ok(Json(doc))
}
fn internal(err: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = PathBuf::from(std::env::var("CHAPTER_FILE").unwrap_or_else(|_| "chapter.json".into()));
    let chapter: Chapter = match tokio::fs::read(&file).await {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Chapter {
            title:"Новая глава".into(),markdown:"Первый абзац.\n\nВторой абзац.".into(),revision:0,
            content_hash:editor_core::hash("Первый абзац.\n\nВторой абзац."),
        },
        Err(e) => return Err(e.into()),
    };
    let state = AppState { chapter:Arc::new(Mutex::new(chapter)),file };
    let app = Router::new().route("/api/chapter",get(read).put(save)).layer(DefaultBodyLimit::max(editor_core::MAX_TEXT_BYTES + 1024 * 1024))
        .fallback_service(ServeDir::new("dist")).with_state(state)
        .layer(CorsLayer::permissive());
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let listener=tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("Open http://127.0.0.1:{port}");
    axum::serve(listener,app).await?;
    Ok(())
}
