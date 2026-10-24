//! Server-side file access: the folder/file picker and audio streaming for the browser player.

use crate::transcode;
use crate::Server;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use lrcget_lib::db;
use lrcget_lib::state::ServiceAccess;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tower_http::services::ServeFile;

/// Resolves `path` (following symlinks) and checks that it lies inside `root`.
pub fn resolve_within_root(root: &Path, path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("No path given".to_owned());
    }
    let resolved = Path::new(path)
        .canonicalize()
        .map_err(|err| format!("Cannot open {path}: {err}"))?;
    if resolved.starts_with(root) {
        Ok(resolved)
    } else {
        Err(format!("{path} is outside of {}", root.display()))
    }
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::Value::String(message.into()))).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    path: Option<String>,
    /// Also list files, not only directories.
    #[serde(default)]
    files: bool,
    /// Comma-separated file extensions to list (without dots); all files when empty.
    extensions: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryListing {
    path: PathBuf,
    /// `None` at the root, which cannot be left.
    parent: Option<PathBuf>,
    entries: Vec<DirectoryEntry>,
}

/// `GET /api/fs?path=...&files=true&extensions=mp3,flac`
pub async fn list_directory(
    State(server): State<Server>,
    Query(query): Query<ListQuery>,
) -> Response {
    let root = &server.config.root;
    let requested = query
        .path
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
    let path = match resolve_within_root(root, &requested) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("{requested} is not a directory"),
            )
        }
        Err(err) => return error_response(StatusCode::BAD_REQUEST, err),
    };

    let extensions: Vec<String> = query
        .extensions
        .unwrap_or_default()
        .split(',')
        .map(|ext| ext.trim().trim_start_matches('.').to_lowercase())
        .filter(|ext| !ext.is_empty() && ext != "*")
        .collect();

    let read_dir = match std::fs::read_dir(&path) {
        Ok(read_dir) => read_dir,
        Err(err) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Cannot read {}: {err}", path.display()),
            )
        }
    };

    let mut entries: Vec<DirectoryEntry> = read_dir
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let entry_path = entry.path();
            // `is_dir` follows symlinks, so linked folders show up as folders.
            let is_dir = entry_path.is_dir();
            if !is_dir {
                if !query.files {
                    return None;
                }
                let extension = entry_path
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if !extensions.is_empty() && !extensions.contains(&extension) {
                    return None;
                }
            }
            Some(DirectoryEntry {
                name,
                path: entry_path,
                is_dir,
            })
        })
        .collect();
    entries.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));

    let parent = if &path == root {
        None
    } else {
        path.parent().map(Path::to_path_buf)
    };

    Json(DirectoryListing {
        path,
        parent,
        entries,
    })
    .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioQuery {
    track_id: Option<i64>,
    path: Option<String>,
    /// Decode to WAV on the server for formats the browser cannot play.
    #[serde(default)]
    transcode: bool,
}

/// `GET /api/audio?trackId=1` or `?path=/music/song.flac`, optionally with `&transcode=true`.
/// Supports HTTP range requests so the browser can seek.
pub async fn audio(
    State(server): State<Server>,
    Query(query): Query<AudioQuery>,
    request: Request,
) -> Response {
    let file_path = match (query.track_id, query.path) {
        (Some(track_id), _) => match server.ctx.db(|db| db::get_track_by_id(track_id, db)) {
            Ok(track) => track.file_path,
            Err(err) => return error_response(StatusCode::NOT_FOUND, err.to_string()),
        },
        (None, Some(path)) => path,
        (None, None) => return error_response(StatusCode::BAD_REQUEST, "Pass trackId or path"),
    };

    let mut path = match resolve_within_root(&server.config.root, &file_path) {
        Ok(path) if path.is_file() => path,
        Ok(_) => {
            return error_response(StatusCode::NOT_FOUND, format!("{file_path} is not a file"))
        }
        Err(err) => return error_response(StatusCode::NOT_FOUND, err),
    };

    if query.transcode {
        let cache_dir = server.config.data_dir.join("transcode-cache");
        path = match transcode::to_wav(path, cache_dir).await {
            Ok(path) => path,
            Err(err) => {
                return error_response(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    format!("Cannot decode {file_path}: {err}"),
                )
            }
        };
    }

    match ServeFile::new(&path).try_call(request).await {
        Ok(response) => response.map(Body::new),
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}
