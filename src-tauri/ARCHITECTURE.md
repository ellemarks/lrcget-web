# LRCGET Tauri Architecture

## Architecture Overview

**Stack:** Tauri v2 (Rust) + SQLite + Kira audio + LRCLIB API, plus an optional axum web server

The crate builds a library (`lrcget_lib`, everything except the binaries) and two binaries behind Cargo features:

| Binary | Feature | Entry | Frontend transport | Audio |
|--------|---------|-------|--------------------|-------|
| `lrcget` (desktop, default) | `desktop` (default) | `src/main.rs` | Tauri IPC | Kira, on the machine running LRCGET |
| `lrcget-server` | `server` | `src/bin/lrcget-server/` | HTTP + WebSocket | `<audio>` in the browser, streamed from the server |

Tauri, its plugins and Kira are optional dependencies enabled by `desktop`, so the server builds without WebKitGTK/ALSA: `cargo build --bin lrcget-server --no-default-features --features server`. `build.rs` only runs `tauri_build` when `desktop` is enabled.

**Patterns:**
| Pattern | Implementation |
|---------|---------------|
| State | `AppState` with `Mutex<Connection>` and queued notifications; the desktop binary keeps `Mutex<Player>` in its own `DesktopState` |
| DB Access | `ServiceAccess` trait on `AppState` (`ctx.db(...)`, `ctx.db_mut(...)`) |
| Events | `Ctx::emit()` → `EventSink` (Tauri `app.emit()` on desktop, WebSocket broadcast on the server) |
| Commands | Implemented once in `commands.rs` as `async fn(ctx: &Ctx, ...)`; listed in `for_each_command!`, from which both binaries generate their wrappers |
| Scanning | Single-pass streaming with batch processing (100 files) |
| Export | Optional TXT/LRC/embedded export after library/album/artist bulk Download; shared manual writers |

**Key Dependencies:** `tauri`, `rusqlite`+`rusqlite_migration`, `lofty`, `kira`, `reqwest`, `rayon`, `xxhash-rust`, `regex` (for LRC parsing), `charabia` (international word segmentation), `fastrand` (retry jitter)

**Word Segmentation Command:** `segment_words(text)` in `commands.rs` uses Charabia's segmenter (`Segment::segment_str`) and then applies language-agnostic post-processing: segments containing at least one letter/number are kept as tokens, while separator-only segments (spaces/punctuation/symbols) are merged into adjacent tokens.

## Project Structure

```
src-tauri/
├── src/
│   ├── lib.rs               # lrcget_lib: module list shared by both binaries
│   ├── main.rs              # Desktop entry point: Tauri command wrappers, Kira player commands, WebDriver plugin wiring
│   ├── commands.rs          # Command implementations (Ctx, EventSink, for_each_command! list)
│   ├── bin/lrcget-server/   # Web server binary (feature `server`)
│   │   ├── main.rs          # Config (env vars), router, static frontend, shutdown
│   │   ├── api.rs           # POST /api/invoke/{command}, GET /api/events (WebSocket), GET /api/info
│   │   ├── files.rs         # GET /api/fs (path picker), GET /api/audio (range streaming), LRCGET_ROOT guard
│   │   ├── jobs.rs          # Server-side bulk download/export queues
│   │   ├── transcode.rs     # Symphonia → cached WAV for formats browsers can't decode
│   │   └── auth.rs          # Optional HTTP basic auth
│   ├── state.rs             # AppState, ServiceAccess trait
│   ├── db.rs                # SQLite operations, migrations
│   ├── library.rs           # High-level library API
│   ├── scanner/             # Incremental file scanning (NEW)
│   │   ├── scan.rs          # Single-pass streaming scanner
│   │   ├── hasher.rs        # xxhash3 content hashing
│   │   ├── metadata.rs      # Audio metadata extraction
│   │   └── models.rs        # ScanResult, ScanProgress
│   ├── parser/              # File format parsers
│   │   └── lrc.rs           # LRC lyrics parser (replaces lrc crate)
│   ├── word_segmentation.rs # Charabia-based tokenization + separator-merging logic/tests
│   ├── export.rs            # Manual sidecar/embed export helpers
│   ├── lyricsfile.rs        # YAML lyricsfile helpers
│   ├── player.rs            # Kira audio playback
│   ├── persistent_entities.rs # Track/Album/Artist structs
│   ├── utils.rs             # Text normalization
│   └── lrclib/              # LRCLIB API client
│       ├── client.rs        # Shared HTTP client, retry/backoff, error model, App ID headers
│       ├── search.rs        # GET /api/search
│       ├── get.rs           # GET /api/get
│       ├── get_by_id.rs     # GET /api/get/{id}
│       ├── publish.rs       # POST /api/publish (with PoW)
│       ├── flag.rs          # POST /api/flag (with PoW)
│       └── challenge_solver.rs # SHA256 proof-of-work
├── migrations/              # SQL files (v1-v9), rusqlite_migration
└── Cargo.toml / tauri.conf.json
```

## Core Components

### State Management (`state.rs`, `commands.rs`)

```rust
struct AppState {
    db: Mutex<Option<Connection>>,           // SQLite
    queued_notifications: Mutex<Vec<Notify>>,
}

trait ServiceAccess {                    // implemented for AppState
    fn db<F, T>(&self, f: F) -> T;      // Read-only
    fn db_mut<F, T>(&self, f: F) -> T;  // Mutable
}

#[derive(Clone)]
struct Ctx {                             // handed to every command, derefs to AppState
    state: Arc<AppState>,
    events: Arc<dyn EventSink>,          // fn emit_value(&self, event: &str, payload: Value)
}
```

`db::initialize_database(app_dir)` takes the data directory: the desktop app passes Tauri's `app_data_dir`, the server `LRCGET_DATA_DIR`.

### Adding a command

1. Implement `pub async fn name(ctx: &Ctx, arg: Type, ...) -> Result<T, E>` in `commands.rs` (argument names become the frontend's camelCase `invoke` keys).
2. Add `name(arg: Type, ...) -> Result<T, E>;` to `for_each_command!` at the bottom of `commands.rs`, using `$crate::` paths for crate types.

Both the Tauri handler and the server's dispatch table are generated from that list. Desktop-only commands (Kira playback, `open_devtools`) live in `main.rs`; the server answers them with 404.

### Database Schema (v10)

**Tables:**
| Table | Purpose |
|-------|---------|
| `directories` | Watched music paths |
| `library_data` | Init flag (single row) |
| `config_data` | Settings (embed, skip flags, theme, LRCLIB instance) |
| `artists` | name, name_lower (search) |
| `albums` | name, album_artist_name, image_path |
| `tracks` | file_path, title, duration, lrc_lyrics, txt_lyrics |
| `lyricsfiles` | Persisted YAML lyrics (decoupled from tracks). Contains track metadata (title, album, artist, duration), presence fields (`has_plain_lyrics`, `has_synced_lyrics`, `has_word_synced_lyrics`, `instrumental`), and optional LRCLIB source fields (`lrclib_instance`, `lrclib_id`). `track_id` is NULL for standalone LRCLIB lyrics without local track association. |

**Migration v8 (scanning):** Added `file_size`, `modified_time`, `content_hash`, `scan_status`

**Migration v9:** Added `lyricsfiles` table with denormalized track metadata

**Migration v10:** Added indexed lyrics-presence booleans on `tracks` (`has_plain_lyrics`, `has_synced_lyrics`, `has_word_synced_lyrics`) used for filtering

**Migration v11:** Added `volume` column to `config_data` for persisting player volume level

**Migration v12:** Added `lrclib_instance` and `lrclib_id` columns to `lyricsfiles` table for tracking remote LRCLIB lyrics sources. Enables editing lyrics from LRCLIB without requiring a local track association.

**Migration v13:** Added `has_plain_lyrics`, `has_synced_lyrics`, `has_word_synced_lyrics`, `instrumental` columns to `lyricsfiles` table. This moves the source of truth for lyrics presence to the `lyricsfiles` table, enabling filtering based on lyricsfile content for both library tracks and standalone LRCLIB lyrics.

**Migration v14:** Recreates the `tracks` table without deprecated lyrics columns (`txt_lyrics`, `lrc_lyrics`, `instrumental`, `has_plain_lyrics`, `has_synced_lyrics`, `has_word_synced_lyrics`). All lyrics data is now stored exclusively in the `lyricsfiles` table. Forces a library reset.

**Migration v15:** Added indexes on `tracks.album_id` and `tracks.artist_id`.

**Migration v17:** Added `auto_export_enabled` (false), `export_lrc` (true), `export_txt` (false), and `export_embedded` (false) to `config_data`. Shared Download/Export defaults persist through a scoped update, without resetting library data. Ordinary `set_config` leaves these fields untouched.

**Migration v16:** Added SQLite FTS5 virtual tables for full-text search:
- `tracks_fts` — indexed `title`, `artist_name`, `album_name` (with `track_id` as UNINDEXED key)
- `albums_fts` — indexed `album_name`, `album_artist_name` (with `album_id` as UNINDEXED key)
- `artists_fts` — indexed `artist_name` (with `artist_id` as UNINDEXED key)
- Backfilled from existing normalized `*_lower` columns.

**Indexes:** All `*_lower` columns + `content_hash`, `scan_status`, `modified_time+file_size` (fingerprint) + lyrics-presence indexes + LRCLIB composite index (`lrclib_instance`, `lrclib_id`)

### File Scanning (`scanner/`)

**ScanResult:** `{ total_files, added, modified, deleted, moved, unchanged, is_initial_scan, duration_ms }`

**ScanProgress:** `{ phase: "discovering"|"updating", progress: f64, files_processed, files_total, message }`

**Process:**
1. Mark existing tracks as "pending" (scan_status=0)
2. Single-pass streaming: discover + process simultaneously
3. Batch size: 100 files
4. Detection modes:
   - **Hash** (default): xxhash3 of first 64KB - detects moves, 100% accurate
   - **Metadata**: mtime+size only - faster, may duplicate on metadata changes
5. Delete remaining "pending" tracks

**Performance:** 100K files HDD ~30-90s faster, SSD ~5-10s faster; Memory ~10MB vs ~200MB old

### FTS5 Search

Library search (tracks, albums, artists) uses SQLite FTS5 virtual tables for tokenized, prefix-matching full-text search. A runtime `fts5_enabled()` check verifies the virtual tables exist; if missing, queries transparently fall back to `LIKE` on `*_lower` columns.

**Virtual Tables:**
| Table | Indexed Columns | Key |
|-------|----------------|-----|
| `tracks_fts` | `title`, `artist_name`, `album_name` | `track_id` (UNINDEXED) |
| `albums_fts` | `album_name`, `album_artist_name` | `album_id` (UNINDEXED) |
| `artists_fts` | `artist_name` | `artist_id` (UNINDEXED) |

**Query Building:** `utils::build_fts_query()` normalizes input (diacritics stripped, punctuation removed, lowercased) and converts each token into a prefix query. Example: `Love Way` → `love* way*`.

**Ordering:** Results are ordered by `rank` (FTS5 relevance) when FTS5 is available; otherwise alphabetical by `*_lower`.

**Sync:** The FTS indexes are kept in sync with the source tables at mutation points:
- **Insert:** `insert_new_track()` inserts into `tracks_fts`; `add_artist_tx()` inserts into `artists_fts`; `add_album_tx()` inserts into `albums_fts`.
- **Delete:** `delete_unprocessed_tracks()` removes pending track IDs from `tracks_fts` and orphaned album/artist IDs from `albums_fts`/`artists_fts`; `clean_library()` clears all three virtual tables.

### Orphaned Lyricsfile Reattachment

When tracks are deleted from the library, their associated `lyricsfiles` records are retained (via `ON DELETE SET NULL`). When the same track is re-added during a library scan, the system automatically reattaches these orphaned lyricsfiles instead of requiring re-import from embedded metadata or sidecar files.

**Matching Criteria:**
- Exact match on normalized `track_artist_name_lower` + `track_title_lower` + `track_album_name_lower`
- Duration must be within ±2 seconds
- Only considers lyricsfiles where `track_id IS NULL` (orphaned)

**Process during `insert_new_track()`:**
1. Extract metadata from audio file
2. Query for matching orphaned lyricsfile via `db::find_orphaned_lyricsfile_tx()`
3. If found: reattach via `db::reattach_lyricsfile_to_track_tx()`, skip embedded lyrics import
4. If not found: proceed with normal embedded/sidecar lyrics import

**Benefits:**
- Preserves lyrics edits when tracks are temporarily removed and re-added
- Avoids redundant re-import from sidecar files
- Works for both previously-deleted tracks and standalone LRCLIB lyrics

### Audio Player (`player.rs`)

```rust
struct Player {
    manager: AudioManager,              // Kira
    sound_handle: Option<StreamingSoundHandle>,
    track: Option<PlayableTrack>,      // Unified type for DB and file-based tracks
    status: PlayerStatus,               // Playing/Paused/Stopped
    progress: f64, duration: f64, volume: f64, playback_speed: f64,
}
```

The player uses `PlayableTrack` to support both library tracks (from database) and arbitrary tracks (from file picker). This enables playback of tracks not present in the library database.

Desktop only (feature `desktop`, managed as `DesktopState` in `main.rs`). In the web version the browser plays audio itself (see the web server section below and `src/composables/player.js`).

Background loop (40ms) in `main.rs` emits `player-state` event.

Volume persistence:
- Player initializes with volume from `config_data` on startup
- `set_volume()` command updates both the player and persists to config
- Frontend receives volume updates via `player-state` events

Playback speed:
- Player tracks current `playback_speed` and reapplies it when a new track starts
- `set_playback_speed()` command updates the active sound handle rate (clamped 0.5x–2.0x)
- Frontend receives playback speed via `player-state` events

### Export Module (`export.rs`)

Shared lyrics export to sidecar files (`.txt`, `.lrc`) and manual embedded metadata export. Automatic Download export calls the same `export_track` writers after successful internal save.

```rust
pub enum ExportFormat {
    Txt,        // Plain text sidecar file
    Lrc,        // Synced LRC sidecar file
    Embedded,   // Embedded in audio metadata (MP3/FLAC)
}

pub struct ExportResult {
    pub format: ExportFormat,
    pub path: Option<PathBuf>,
    pub success: bool,
    pub message: String,
}
```

**Key Functions:**
- `export_track()` - Export a single track to multiple formats
- `export_track_format()` - Export to a specific format
- `embed_lyrics()` - Embed lyrics into MP3 (ID3v2 USLT/SYLT) or FLAC (Vorbis comments)

**Note:** Sidecar exports retain their existing semantics: overwrite the selected target and attempt to delete the opposite extension before writing available content. Unavailable formats skip before deletion. Bulk export runs TXT, LRC, then embedded, so both successful sidecar operations normally leave only LRC. Failures do not roll back filesystem changes. Embedded exports use the existing `lofty` MP3/FLAC writers. The shared format boundary reports unsupported extensions as `Skipped` before touching the file. Both manual commands and automatic export use `export_track_with_embed_gate`, reading current `try_embed_lyrics` immediately before each embedded operation (read failures deny embedding). A disabled gate produces a format-level skip, including embed-only batches whose gate was disabled after submission; successful internal downloads retain their success string. Preference submission rejects zero effective formats and ignores embedded-choice updates while the gate is disabled. Supported-writer edge cases remain unchanged: MP3 requires an existing primary tag; FLAC without Vorbis comments currently performs no tag update.

### LRC Parser (`parser/lrc.rs`)

Lightweight LRC (LyRiCs) format parser that replaces the external `lrc` crate. Supports timestamp tags with 1-3 digit precision for milliseconds.

**Problem with `lrc` crate:** The original crate only supported 1-2 digit fractional seconds (centiseconds) and silently ignored lines with 3-digit milliseconds, causing data loss when parsing LRC files like `[01:35.492] Line text`.

**Solution:** Custom parser with flexible timestamp parsing:
- `[mm:ss.x]` - 1 digit = deciseconds (×100ms)
- `[mm:ss.xx]` - 2 digits = centiseconds (×10ms)
- `[mm:ss.xxx]` - 3 digits = milliseconds (×1ms)

```rust
pub struct TimedLine {
    pub timestamp_ms: i64,
    pub text: String,
}

pub struct ParsedLrc {
    pub timed_lines: Vec<TimedLine>,
    pub id_tags: Vec<(String, String)>,
}

pub fn parse_lrc(input: &str) -> ParsedLrc;
pub fn is_instrumental_lrc(input: &str) -> bool;
pub fn format_timestamp(timestamp_ms: i64) -> String;
```

**Features:**
- Multiple timestamps per line: `[00:01.00][00:03.00] Repeated text`
- ID tags: `[ti:Title]`, `[ar:Artist]`, `[au:instrumental]`
- Automatic sorting by timestamp
- Instrumental detection via `[au:instrumental]` marker

**Used by:**
- `lyricsfile.rs` - Parsing embedded LRC lyrics during import
- `export.rs` - Converting LRC to SYLT format for MP3 embedding

### LRCLIB API (`lrclib/`)

**Endpoints:** search, get, get_by_id, publish, flag, request_challenge

**Shared HTTP client (`client.rs`):**
- A single process-wide `reqwest::Client` (10s timeout) is reused for connection pooling.
- Sets `User-Agent: LRCGET v<version> (GitHub URL)` on every request.
- Sends the public `Lrclib-App-Id` header when the configured instance is `lrclib.net` or a subdomain. This gives the registered client higher rate limits (~180 req/min per IP plus an app-wide allowance). The App ID is not a secret; server-side `Lrclib-Api-Key` is never embedded.
- The App ID is a compile-time constant in `client.rs`. Forks should register their own application and replace it.
- Custom/self-hosted instances do not receive the App ID, because they may reject an ID registered on another instance.
- Provides `get`, `post_json`, and `post` helpers plus shared endpoint/URL construction.
- Sends `Accept: application/json` on every request.

**Query hygiene:**
- Empty optional parameters are omitted.
- `/api/get`: always sends `track_name` and `artist_name` (validated client-side); sends `album_name` only when non-empty and `duration` only when it rounds into the documented 1–3600 range.
- `/api/search`: when `q` is non-empty it is sent alone (it takes precedence); otherwise `track_name` is sent with non-empty `artist_name`/`album_name`. A search with no `q` and no title returns an empty result without a network request.
- Response models include `id`, `trackName`, and `hasWordSync`; `hasWordSync` defaults to `false` for older instances that omit it.

**Error model:**
- `LrclibError` carries a typed `kind` (`NotFound`, `Validation`, `Unauthorized`, `RateLimited`, `ServerOverloaded`, `ServerError`, `Network`, `Unexpected`), HTTP status, API `name`/`message`, `Retry-After`, and rate-limit `scope`.
- Parses the current `{ message, name, statusCode }` schema and accepts the legacy `error` field for older instances.
- Non-JSON responses (for example network-edge errors) fall back to a truncated text preview instead of failing to decode.
- `send_with_retry()` converts non-2xx responses into `LrclibError` and applies the retry policy.
- `parse_json()` reports invalid success bodies explicitly.

**Retry policy (`RetryPolicy`):**
- **Batch** (mass-download `/api/get`): 4 attempts total. Retries 429, 500, 502, 503, 504, timeouts, and connection errors. Honors `Retry-After` plus up to 30% jitter; otherwise uses 500 ms → 1 s → 2 s exponential backoff capped at 8 s.
- **Interactive** (search, preview, lyrics by ID): 3 attempts total for quick 5xx/network failures. A 429 fails fast rather than blocking the UI; a 503 with a `Retry-After` longer than 3 s also fails fast.
- **NoRetry** (publish/flag): one attempt only, because `X-Publish-Token` is single-use.
- **Shared cooldown:** a 429 (or a 503 carrying `Retry-After`) sets a process-wide cooldown. Batch requests wait for it; interactive and one-shot requests return a wait error instead of sending a request early.

**Challenge-Response (publish/flag):**
1. Request challenge → prefix + target hash
2. Solve PoW (SHA256): find nonce where hash(prefix+nonce) < target
3. Submit with token

**Write retry behavior:**
- The HTTP layer uses `RetryPolicy::NoRetry` for publish/flag because `X-Publish-Token` is single-use.
- `publish_lyrics` / `flag_lyrics` retry up to 3 times on an explicit 429 or 503 by requesting a **new** challenge, re-solving PoW, and submitting a **fresh** token.
- `Retry-After` is honored with jitter when the requested wait is ≤ 5 s; longer waits fail fast with a clear message instead of blocking the modal.
- 500/502/504 responses and timeouts are not retried, because the write may already have been processed.

### Data Entities (`persistent_entities.rs`)

**PersistentTrack:** id, file_path, file_name, title, album_name, artist_name, album_id, artist_id, image_path, track_number, txt_lyrics, lrc_lyrics, lyricsfile, duration, instrumental

**PlayableTrack:** A unified type for playback that works with both database tracks and arbitrary file-based tracks. Used by the `Player` to support:
- Library tracks (from database): `id` is `Some(track_id)`
- File picker tracks: `id` is `None`, metadata provided directly

Implements `From<PersistentTrack>` for seamless conversion from database entities.

**PersistentAlbum:** id, name, artist_name, tracks_count

**PersistentArtist:** id, name, tracks_count

**PersistentConfig:** skip_synced, skip_plain, show_line_count, try_embed, theme_mode, lrclib_instance, volume, auto_export_enabled, export_lrc, export_txt, export_embedded

## Web Server (`src/bin/lrcget-server/`)

Serves the built frontend (`LRCGET_WEB_DIR`, SPA fallback to `index.html`) and exposes the shared commands so LRCGET can run headless and be used from any browser. Configured with environment variables (`lrcget-server --help`): `LRCGET_BIND`, `LRCGET_DATA_DIR`, `LRCGET_WEB_DIR`, `LRCGET_ROOT`, `LRCGET_MUSIC_DIR`, `LRCGET_USERNAME`/`LRCGET_PASSWORD`.

| Route | Purpose |
|-------|---------|
| `POST /api/invoke/{command}` | JSON object of camelCase args → JSON result. Errors return the command's error value as JSON (400), so the frontend rejects exactly like Tauri's `invoke`. Bad args 422, unknown command 404 |
| `GET /api/events` | WebSocket; every `Ctx::emit` is broadcast to all clients as `{ "event", "payload" }` |
| `GET /api/info` | `{ version, root, musicDir }` (replaces Tauri's `getVersion` / `audioDir`) |
| `GET /api/fs?path=&files=&extensions=` | Directory listing for the browser path picker |
| `GET /api/audio?trackId=` or `?path=` (`&transcode=true`) | Audio file with HTTP range support; `transcode` decodes with Symphonia into a cached 16-bit WAV (`<data dir>/transcode-cache`, newest 10 kept) for formats the browser can't play, e.g. ALAC |

- **`LRCGET_ROOT`:** `/api/fs`, `/api/audio`, `read_text_file` and `get_audio_metadata` only accept paths that canonicalize (following symlinks) to somewhere under it. Defaults to `/`; the Docker image sets `/music`.
- **Auth:** with `LRCGET_USERNAME` and `LRCGET_PASSWORD` set, every request needs HTTP basic auth (browsers reuse it for `<audio>` and the WebSocket). The server warns at startup when bound to a non-loopback address without credentials.
- **`set_volume`** only persists the volume; playback commands (`play_track`, `pause_track`, ...) are not available.
- **Bulk queues (`jobs.rs`):** the desktop app runs Download/Export batches as loops in the frontend; in a browser those stall in background tabs, so the server runs them instead. Extra commands: `queue_downloads(trackIds, autoExport?)`, `get_download_queue()`, `reset_download_queue()`, `queue_exports(trackIds, formats)`, `get_export_queue()`, `reset_export_queue()`. Each worker is a Tokio task fed by a `Notify`. The download worker keeps the frontend's spacing (500 ms + 0–250 ms jitter) and pauses on `rateLimited`/`serverOverloaded` errors for `retryAfterSecs`. Every change broadcasts a full snapshot (`download-queue-state` / `export-queue-state`: counts, `cooldownRemaining`, last 100 log entries). Resetting bumps a generation counter, so a request that was already in flight isn't counted in the next batch.
- **Docker:** root `Dockerfile` (Node 24 frontend build → `cargo build --release --features server` → `debian:bookworm-slim` runtime as UID 1000) and an example `docker-compose.yml`.

## WebDriver Dev Bridge (`tauri-plugin-webdriver`)

Agent-driven inspection (screenshots, DOM queries, JS eval, interaction, logs) via a [W3C WebDriver](https://www.w3.org/TR/webdriver/) server, provided by [tauri-plugin-webdriver](https://github.com/Choochmeque/tauri-plugin-webdriver).

- **Debug builds only**: the dependency lives under `[target.'cfg(debug_assertions)'.dependencies]` in `Cargo.toml`, and the plugin is registered with `#[cfg(debug_assertions)] let builder = builder.plugin(tauri_plugin_webdriver::init());` in `main.rs`. Release builds have no server and no plugin code.
- **Transport**: HTTP server on `127.0.0.1:4445` (override with `TAURI_WEBDRIVER_PORT` env var or `init_with_port`). Drive it with any W3C WebDriver client (Selenium, WebdriverIO, `fantoccini`).
- **Linux backend**: WebKitGTK native APIs (`webkit_web_view_get_snapshot` for screenshots, `evaluate_javascript_future` for scripts) — compositor-independent, works on X11 and any Wayland compositor. Element screenshots are a full-webview snapshot after `scrollIntoView` (no pixel crop on Linux).
- **No custom IPC commands**: unlike the previous tauri-agent-tools bridge, nothing is added to the `invoke_handler` list, so there is no debug/release duplication to keep in sync.
- **How to run and use it** (launch recipe, session flow, caveats): see *"Inspecting a running dev build"* in the root `AGENTS.md`.

## Commands

### Library
| Command | Description |
|---------|-------------|
| `scan_library(use_hash?)` | Incremental scan. Emits `scan-progress`, `scan-complete` |
| `uninitialize_library()` | Clear all library data |

### Data Queries
| Command | Returns |
|---------|---------|
| `get_track(s/ids)(search_query?)` | `PersistentTrack` (includes joined `lyricsfile`) |
| `get_album(s/ids)(search_query?)` | `PersistentAlbum` |
| `get_artist(s/ids)(search_query?)` | `PersistentArtist` |
| `get_album_tracks/ids()` | Filtered tracks |
| `get_artist_tracks/ids()` | Filtered tracks |

Search across all three entity types uses SQLite FTS5 (via `tracks_fts`, `albums_fts`, `artists_fts`). Results are ordered by FTS5 `rank` (relevance). A runtime `fts5_enabled()` check falls back to `LIKE` queries on `*_lower` columns if the virtual tables are unavailable.

### Lyrics
| Command | Purpose |
|---------|---------|
| `download_lyrics(track_id, auto_export?)` | Download from LRCLIB using the batch retry policy and save internally. Optional explicit `{ plainText, syncedLrc, embedIntoTrack }` batch snapshot enables TXT/LRC/embedded export after save; empty selections are rejected before downloading. Returns the existing string appended with ordered export outcomes/errors, or rejects with a structured `DownloadLyricsError` (`kind`, `message`, `retryAfterSecs`, `scope`) on failure so the frontend can pause the queue on rate limits. Omitted options preserve internal-only behavior. |
| `retrieve_lyrics/by_id()` | Get raw LRCLIB response |
| `search_lyrics()` | Search LRCLIB database |
| `apply_lyrics()` | Save a selected LRCLIB result into database-backed lyrics storage |
| `prepare_lrclib_lyricsfile(lrclib_id)` | Get or create lyricsfile from LRCLIB. Checks local cache first, fetches from API if needed. Saves to `lyricsfiles` table with `lrclib_instance` + `lrclib_id`. Returns `lyricsfile_id` + content + `exists_in_db` flag. |
| `refresh_lrclib_lyricsfile(lrclib_id)` | Force re-download lyrics from LRCLIB API. Updates existing record in `lyricsfiles` table. Returns refreshed `lyricsfile_id` + content. |
| `save_lyrics(track_id?, lyricsfile_id?, plain?, synced?, lyricsfile?)` | Save lyrics edits. For library tracks: provide `track_id`. For standalone LRCLIB lyrics: provide `lyricsfile_id`. Prefers `lyricsfile` format. |
| `publish_lyrics(title, album, artist, duration, plain?, synced?, lyricsfile?)` | Upload to LRCLIB (with PoW; accepts Lyricsfile-only payloads). Retries explicit 429/503 with a fresh challenge/token |
| `export_lyrics(track_id, formats, lyricsfile?)` | Manual export to `.txt`, `.lrc`, or embedded tags |
| `export_track_lyrics(track_id, formats)` | Export single track, returns summary for mass export |
| `get_track_ids_with_lyrics()` | Get all track IDs that have lyrics for mass export |
| `flag_lyrics()` | Report to LRCLIB (with PoW). Sends the documented `content` field and retries explicit 429/503 with a fresh token |
| `find_matching_tracks(title, album, artist, duration?)` | Find local tracks matching LRCLIB metadata (for My LRCLIB edit flow) |
| `get_audio_metadata(filePath)` | Extract metadata from audio file (for file picker) |
| `prepare_search_query(title)` | Prepare search query by removing brackets and normalizing |

### Playback & Config
- `play_track(track_id?, file_path?, title?, album_name?, artist_name?, album_artist_name?, duration?)` - Unified playback for both library tracks (via `track_id`) and file-based tracks (via `file_path` with metadata)
- `pause/resume_track()`, `seek_track()`, `stop_track()`, `set_volume()` (persists volume to config), `set_playback_speed()`
- `get/set_directories()`, `get/set_config()`, `get_init()`
- `set_export_preferences(auto_export_enabled?, export_lrc?, export_txt?, export_embedded?, skip_tracks_with_synced_lyrics?, skip_tracks_with_plain_lyrics?)` updates only supplied fields in `config_data` and returns config. Download atomically supplies its existing skip flags, toggle, and effective format selections; manual Export supplies shared formats only. Hidden/omitted fields and unrelated settings survive.
- Volume is loaded from config on startup and auto-saved when changed via `set_volume()`
- `open_devtools()`, `drain_notifications()`

## Events (Backend → Frontend)

| Event | Payload | Purpose |
|-------|---------|---------|
| `scan-progress` | `{ phase, progress, filesProcessed, filesTotal, message }` | Real-time scan updates |
| `scan-complete` | `{ totalFiles, added, modified, deleted, moved, unchanged, isInitialScan, durationMs }` | Scan finished |
| `player-state` | Player status | Playback updates (40ms loop) |
| `reload-track-id` | track_id | Request refresh |
| `publish-lyrics-progress` | Status | Publishing updates |
| `flag-lyrics-progress` | Status | Flagging updates |


## Configuration

**tauri.conf.json:** Window 1024x768 min, CSP for asset protocol + media

**Cargo.toml:** Rust 2021, Tauri v2 (dialog, shell, global-shortcut, os plugins), features `desktop` (default) and `server` (axum, tower-http)

## Track Matching (`find_matching_tracks`)

Backend command for finding local tracks that match LRCLIB metadata. Uses `prepare_input()` normalization for case-insensitive matching.

**Algorithm:**
1. **Strong Match**: Title + Artist + Album match (via `prepare_input()` normalization) AND duration within ±2 seconds (if provided)
2. **Partial Match**: Title matches, returned if no strong matches found

**Types:**
```rust
enum MatchQuality { Strong, Partial }
struct MatchingTrack {
    #[serde(flatten)]
    track: PersistentTrack,
    match_quality: MatchQuality,
}
```

**DB Function:** `db::find_tracks_by_metadata()` searches using normalized `*_lower` columns for case-insensitive matching.

**Note:** The frontend currently uses a simpler approach with `get_tracks` + client-side filtering in `AssociateTrackModal.vue`.

## Audio Metadata Extraction (`get_audio_metadata`)

Extracts metadata from an audio file path using the existing scanner logic. Used by the file picker in the track association flow.

**Returns:**
```rust
struct AudioMetadataResponse {
    file_path: String,
    file_name: String,
    title: String,
    album: String,
    artist: String,
    album_artist: String,
    duration: f64,
    track_number: Option<u32>,
}
```

**Implementation:** Reuses `scanner::metadata::TrackMetadata::from_path()` from the scanning module.

## Search Query Preparation (`prepare_search_query`)

Prepares a search query from track title by:
1. Removing content inside `()` and `[]` brackets (including the brackets)
2. Applying `prepare_input()` normalization (lowercase, secular normalization, special char removal, whitespace collapsing)

**Example:**
- Input: `title="Love The Way You Lie (Remix) [Explicit]"`
- Output: `"love the way you lie"`

Used by `AssociateTrackModal.vue` to prefill the search input when associating LRCLIB tracks with local tracks.

## Testing

Run the backend test suite from `src-tauri/`:

```bash
cargo test
# or, without the Tauri system dependencies:
cargo test --lib --no-default-features
```

`lrclib::client` includes in-process HTTP integration tests backed by a raw `TcpListener` mock server. They cover:

- 429, 500, and 503 responses followed by success
- The 4-attempt Batch retry limit
- No retry for 400/404, interactive 429s, or `NoRetry` write requests
- `Accept: application/json` on requests
- No `Lrclib-App-Id` sent to custom/self-hosted instances

## Notes

- **Lyrics Storage:** `lyricsfiles` table remains the source of truth. The shared confirmed library/album/artist Download popup can opt into TXT/LRC/embedded export after save; all other save paths remain manual export. Automatic export parses the just-saved snapshot without holding the DB lock during file I/O, and failures remain text in the successful download response. The `tracks` table no longer contains `txt_lyrics` or `lrc_lyrics` columns as of migration v14.
- **Filtering Source of Truth:** Lyrics filters use `lyricsfiles.has_*_lyrics` booleans (via LEFT JOIN with COALESCE)
- **Instrumental:** `[au: instrumental]` marker in lrc_lyrics, stored as `instrumental = true` in lyricsfiles table
- **Security:** PoW for LRCLIB writes, registered `Lrclib-App-Id` (public) plus User-Agent in requests, DB in app_data_dir

## Deprecated Code Cleanup

The following deprecated functions were removed from `db.rs` (no longer used):
- `set_track_lyrics_presence()` / `set_track_lyrics_presence_tx()` - No-ops after presence fields moved to `lyricsfiles` table
- `update_track_synced_lyrics()` - Replaced by `upsert_lyricsfile_for_track()`
- `update_track_plain_lyrics()` - Replaced by `upsert_lyricsfile_for_track()`
- `update_track_null_lyrics()` - Replaced by `delete_lyricsfile_by_track_id()`
- `update_track_instrumental()` - Replaced by `upsert_lyricsfile_for_track()` with instrumental lyricsfile content
- `derive_lyrics_presence_from_legacy()` - No longer needed; presence is calculated from lyricsfile content via `lyrics_presence_from_lyricsfile()`

**Scanning Changes:**
- `insert_track_from_metadata_tx()` no longer stores `txt_lyrics`, `lrc_lyrics`, `instrumental`, or presence fields in the `tracks` table
- Embedded lyrics are now exclusively stored in the `lyricsfiles` table via `upsert_lyricsfile_for_track_tx()`
- The tracks table schema for new databases (post-reset) will not include these deprecated columns

**Migration v14:** Uses `ALTER TABLE DROP COLUMN` (SQLite 3.35.0+) to remove deprecated columns: `txt_lyrics`, `lrc_lyrics`, `instrumental`, `has_plain_lyrics`, `has_synced_lyrics`, `has_word_synced_lyrics`. Drops related indexes and forces a library reset.
