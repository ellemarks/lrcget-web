use crate::files::resolve_within_root;
use crate::Server;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use lrcget_lib::commands::{self, Ctx};
use lrcget_lib::for_each_command;
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

/// Why a command did not produce a result. The frontend rejects its `invoke()` promise with the
/// JSON body, exactly like the desktop app rejects with the command's error value.
pub enum CommandError {
    /// The command ran and returned `Err`.
    Failed(Value),
    BadRequest(String),
    NotFound(String),
}

impl From<serde_json::Error> for CommandError {
    fn from(err: serde_json::Error) -> Self {
        CommandError::Failed(Value::String(err.to_string()))
    }
}

impl IntoResponse for CommandError {
    fn into_response(self) -> Response {
        match self {
            CommandError::Failed(value) => (StatusCode::BAD_REQUEST, Json(value)).into_response(),
            CommandError::BadRequest(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(Value::String(message)),
            )
                .into_response(),
            CommandError::NotFound(message) => {
                (StatusCode::NOT_FOUND, Json(Value::String(message))).into_response()
            }
        }
    }
}

/// Generates `dispatch_shared_command`, which deserializes the camelCase arguments of every
/// command in `lrcget_lib::commands` and calls it.
macro_rules! server_dispatch {
    ($( $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => {
        async fn dispatch_shared_command(
            ctx: &Ctx,
            name: &str,
            args: Value,
        ) -> Result<Value, CommandError> {
            match name {
                $(
                    stringify!($name) => {
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        #[allow(dead_code)]
                        struct Args {
                            $($arg: $ty,)*
                        }
                        #[allow(unused_variables)]
                        let Args { $($arg,)* } = serde_json::from_value(args).map_err(|err| {
                            CommandError::BadRequest(format!("Invalid arguments for {name}: {err}"))
                        })?;
                        let result: $ret = commands::$name(ctx, $($arg),*).await;
                        match result {
                            Ok(value) => Ok(serde_json::to_value(value)?),
                            Err(err) => Err(CommandError::Failed(serde_json::to_value(err)?)),
                        }
                    }
                )*
                _ => Err(CommandError::NotFound(format!("Unknown command: {name}"))),
            }
        }
    };
}

for_each_command!(server_dispatch);

fn parse_args<T: serde::de::DeserializeOwned>(
    command: &str,
    args: Value,
) -> Result<T, CommandError> {
    serde_json::from_value(args)
        .map_err(|err| CommandError::BadRequest(format!("Invalid arguments for {command}: {err}")))
}

/// Commands that take a file path argument, which must stay inside `LRCGET_ROOT`.
const FILE_PATH_COMMANDS: &[&str] = &["read_text_file", "get_audio_metadata"];

/// `POST /api/invoke/{command}` with the command's arguments as a JSON object.
pub async fn invoke(
    State(server): State<Server>,
    Path(command): Path<String>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, CommandError> {
    let args = match body {
        Some(Json(Value::Null)) | None => json!({}),
        Some(Json(args)) => args,
    };
    let ctx = &server.ctx;

    if FILE_PATH_COMMANDS.contains(&command.as_str()) {
        let path = args
            .get("filePath")
            .and_then(Value::as_str)
            .unwrap_or_default();
        resolve_within_root(&server.config.root, path)
            .map_err(|err| CommandError::Failed(Value::String(err)))?;
    }

    let result = match command.as_str() {
        // The browser plays audio itself; only the volume preference is kept on the server.
        "set_volume" => {
            let volume = args
                .get("volume")
                .and_then(Value::as_f64)
                .ok_or_else(|| CommandError::BadRequest("Missing volume".to_owned()))?;
            commands::set_volume_config(ctx, volume)
                .map_err(|err| CommandError::Failed(err.into()))?;
            Value::Null
        }
        "drain_notifications" => serde_json::to_value(commands::drain_notifications(ctx))?,
        // Bulk download/export queues run on the server (see jobs.rs).
        "queue_downloads" => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Args {
                track_ids: Vec<i64>,
                auto_export: Option<commands::AutoExportOptions>,
            }
            let Args {
                track_ids,
                auto_export,
            } = parse_args(&command, args)?;
            server.jobs.queue_downloads(track_ids, auto_export);
            Value::Null
        }
        "get_download_queue" => serde_json::to_value(server.jobs.download_snapshot())?,
        "reset_download_queue" => {
            server.jobs.reset_downloads();
            Value::Null
        }
        "queue_exports" => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Args {
                track_ids: Vec<i64>,
                formats: Vec<commands::ExportLyricsFormat>,
            }
            let Args { track_ids, formats } = parse_args(&command, args)?;
            server.jobs.queue_exports(track_ids, formats);
            Value::Null
        }
        "get_export_queue" => serde_json::to_value(server.jobs.export_snapshot())?,
        "reset_export_queue" => {
            server.jobs.reset_exports();
            Value::Null
        }
        "play_track" | "pause_track" | "resume_track" | "seek_track" | "stop_track"
        | "set_playback_speed" | "open_devtools" => {
            return Err(CommandError::NotFound(format!(
                "{command} is not available in the web version"
            )))
        }
        _ => dispatch_shared_command(ctx, &command, args).await?,
    };

    Ok(Json(result))
}

/// `GET /api/events`: WebSocket stream of `{ "event": ..., "payload": ... }` messages.
pub async fn events(State(server): State<Server>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| forward_events(socket, server))
}

async fn forward_events(mut socket: WebSocket, server: Server) {
    let mut receiver = server.events.subscribe();
    loop {
        tokio::select! {
            event = receiver.recv() => match event {
                Ok(message) => {
                    if socket.send(Message::Text(message.into())).await.is_err() {
                        break;
                    }
                }
                // A slow client missed some events (e.g. scan progress); keep going.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// `GET /api/info`: details the frontend needs that the desktop app gets from Tauri APIs.
pub async fn info(State(server): State<Server>) -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "root": server.config.root,
        "musicDir": server.config.music_dir.as_ref().filter(|dir| dir.is_dir()),
    }))
}
