//! LRCGET web server: serves the Vue frontend and exposes the same commands as the desktop app
//! over HTTP, so LRCGET can run headless (e.g. on a NAS) and be used from any browser.
//!
//! Configuration is read from environment variables, see `Config::from_env`.

mod api;
mod auth;
mod files;
mod jobs;
mod transcode;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use lrcget_lib::commands::{Ctx, EventSink};
use lrcget_lib::db;
use lrcget_lib::state::AppState;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::services::{ServeDir, ServeFile};

const HELP: &str = "\
LRCGET web server

Usage: lrcget-server

Configuration (environment variables):
  LRCGET_BIND        Address to listen on (default: 127.0.0.1:3000)
  LRCGET_DATA_DIR    Where the database and caches live
                     (default: $XDG_DATA_HOME/lrcget-server or ~/.local/share/lrcget-server)
  LRCGET_WEB_DIR     Built frontend (`npm run build` output) to serve (default: ./dist next to
                     the binary, falling back to the repository's dist/ directory)
  LRCGET_ROOT        Only files and folders under this path can be browsed, played or read
                     (default: /)
  LRCGET_MUSIC_DIR   Directory suggested when setting up a new library
                     (default: LRCGET_ROOT if set, otherwise ~/Music)
  LRCGET_USERNAME    Together with LRCGET_PASSWORD, require HTTP basic auth for every request
  LRCGET_PASSWORD
";

pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub web_dir: PathBuf,
    pub root: PathBuf,
    pub music_dir: Option<PathBuf>,
    pub credentials: Option<(String, String)>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let home = var("HOME").map(PathBuf::from);

        let bind = var("LRCGET_BIND")
            .unwrap_or_else(|| "127.0.0.1:3000".to_owned())
            .parse()
            .map_err(|err| format!("Invalid LRCGET_BIND: {err}"))?;

        let data_dir = var("LRCGET_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| var("XDG_DATA_HOME").map(|dir| PathBuf::from(dir).join("lrcget-server")))
            .or_else(|| {
                home.as_ref()
                    .map(|home| home.join(".local/share/lrcget-server"))
            })
            .unwrap_or_else(|| PathBuf::from("lrcget-data"));

        let web_dir = var("LRCGET_WEB_DIR").map(PathBuf::from).unwrap_or_else(|| {
            let next_to_binary = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("dist")));
            match next_to_binary {
                Some(dir) if dir.is_dir() => dir,
                _ => PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../dist")),
            }
        });

        let root_var = var("LRCGET_ROOT");
        let root = PathBuf::from(root_var.clone().unwrap_or_else(|| "/".to_owned()))
            .canonicalize()
            .map_err(|err| format!("Invalid LRCGET_ROOT: {err}"))?;

        let music_dir = var("LRCGET_MUSIC_DIR")
            .map(PathBuf::from)
            .or_else(|| root_var.map(|_| root.clone()))
            .or_else(|| home.map(|home| home.join("Music")));

        let credentials = match (var("LRCGET_USERNAME"), var("LRCGET_PASSWORD")) {
            (Some(username), Some(password)) => Some((username, password)),
            (None, None) => None,
            _ => return Err("Set both LRCGET_USERNAME and LRCGET_PASSWORD, or neither".to_owned()),
        };

        Ok(Self {
            bind,
            data_dir,
            web_dir,
            root,
            music_dir,
            credentials,
        })
    }
}

/// Shared state for all request handlers.
#[derive(Clone)]
pub struct Server {
    pub ctx: Ctx,
    pub config: Arc<Config>,
    pub events: broadcast::Sender<String>,
    pub jobs: Arc<jobs::Jobs>,
}

/// Broadcasts backend events to every connected browser over `/api/events`.
struct BroadcastEvents(broadcast::Sender<String>);

impl EventSink for BroadcastEvents {
    fn emit_value(&self, event: &str, payload: serde_json::Value) {
        let message = serde_json::json!({ "event": event, "payload": payload }).to_string();
        // An error only means no browser is connected right now.
        let _ = self.0.send(message);
    }
}

#[tokio::main]
async fn main() {
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        print!("{HELP}");
        return;
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    if let Err(err) = check_data_dir_writable(&config.data_dir) {
        eprintln!("{err}");
        std::process::exit(1);
    }
    let db = db::initialize_database(&config.data_dir).expect("Database initialize should succeed");
    let (events, _) = broadcast::channel(1024);
    let ctx = Ctx::new(
        Arc::new(AppState::new(db)),
        Arc::new(BroadcastEvents(events.clone())),
    );

    if !config.web_dir.join("index.html").is_file() {
        eprintln!(
            "Warning: no frontend found at {} (run `npm run build` or set LRCGET_WEB_DIR)",
            config.web_dir.display()
        );
    }
    if config.credentials.is_none() && !config.bind.ip().is_loopback() {
        eprintln!(
            "Warning: listening on {} without authentication. Anyone who can reach this address \
             can browse {} and modify your library. Set LRCGET_USERNAME and LRCGET_PASSWORD, or \
             put LRCGET behind an authenticating reverse proxy.",
            config.bind,
            config.root.display()
        );
    }

    let bind = config.bind;
    let web_dir = config.web_dir.clone();
    let server = Server {
        jobs: jobs::Jobs::start(ctx.clone()),
        ctx,
        config: Arc::new(config),
        events,
    };

    let api = Router::new()
        .route("/invoke/{command}", post(api::invoke))
        .route("/events", get(api::events))
        .route("/info", get(api::info))
        .route("/fs", get(files::list_directory))
        .route("/audio", get(files::audio))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024));

    // Unknown paths fall back to index.html so the single-page app can load.
    let frontend =
        ServeDir::new(&web_dir).not_found_service(ServeFile::new(web_dir.join("index.html")));

    let app = Router::new()
        .nest("/api", api)
        .fallback_service(frontend)
        .layer(axum::middleware::from_fn_with_state(
            server.clone(),
            auth::require_basic_auth,
        ))
        .with_state(server);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .unwrap_or_else(|err| panic!("Failed to listen on {bind}: {err}"));
    println!("LRCGET web server listening on http://{bind}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("Server error");
}

/// Fails with an actionable message when the data directory can't be written, the usual cause
/// being a Docker bind mount that Docker created as root.
fn check_data_dir_writable(data_dir: &Path) -> Result<(), String> {
    let probe = data_dir.join(".write-test");
    let result = std::fs::create_dir_all(data_dir)
        .and_then(|_| std::fs::write(&probe, b""))
        .and_then(|_| std::fs::remove_file(&probe));
    let Err(err) = result else {
        return Ok(());
    };

    let mut message = format!(
        "Cannot write to the data directory {} ({err}).",
        data_dir.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let process_uid = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("Uid:"))
                    .and_then(|ids| ids.split_whitespace().next().map(str::to_owned))
            });
        if let (Some(uid), Ok(metadata)) = (process_uid, std::fs::metadata(data_dir)) {
            message.push_str(&format!(
                "\nLRCGET runs as UID {uid}, but the directory is owned by UID {}:{}.",
                metadata.uid(),
                metadata.gid()
            ));
        }
    }
    message.push_str(
        "\nIf it is a Docker bind mount, create the host directory yourself before starting the \
         container (Docker creates missing ones as root), or give it to the container's user, \
         e.g. `sudo chown 1000:1000 ./data`. Otherwise set LRCGET_DATA_DIR to a writable \
         directory.",
    );
    Err(message)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    println!("Shutting down");
}
