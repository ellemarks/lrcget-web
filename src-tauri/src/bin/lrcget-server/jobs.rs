//! Bulk download and export queues, run on the server so they keep going when the browser tab is
//! in the background (where timers are throttled) or closed. The desktop app runs the same queues
//! in the frontend (`src/composables/downloader.js`, `export.js`); this mirrors their behaviour.
//!
//! Every change is broadcast as a full snapshot (`download-queue-state` / `export-queue-state`),
//! so all open tabs show the same progress.

use lrcget_lib::commands::{self, AutoExportOptions, Ctx, ExportLyricsFormat};
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const LOG_LIMIT: usize = 100;
/// Pause between LRCLIB requests, plus up to `REQUEST_JITTER_MS` of random jitter.
const REQUEST_SPACING_MS: u64 = 500;
const REQUEST_JITTER_MS: u64 = 250;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    status: &'static str,
    title: String,
    artist_name: String,
    message: String,
}

#[derive(Default)]
struct DownloadQueue {
    pending: VecDeque<(i64, Option<AutoExportOptions>)>,
    log: VecDeque<LogEntry>,
    active: bool,
    total: u64,
    downloaded: u64,
    succeeded: u64,
    failed: u64,
    cooldown_until: Option<Instant>,
    /// Bumped on reset, so a request that was in flight doesn't count towards the new batch.
    generation: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSnapshot {
    is_downloading: bool,
    queued: usize,
    total_count: u64,
    downloaded_count: u64,
    success_count: u64,
    failure_count: u64,
    cooldown_remaining: u64,
    log: Vec<LogEntry>,
}

#[derive(Default)]
struct ExportQueue {
    pending: VecDeque<(i64, Arc<[ExportLyricsFormat]>)>,
    log: VecDeque<LogEntry>,
    active: bool,
    total: u64,
    exported: u64,
    skipped: u64,
    errors: u64,
    generation: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSnapshot {
    is_exporting: bool,
    queued: usize,
    total_count: u64,
    exported_count: u64,
    skipped_count: u64,
    error_count: u64,
    log: Vec<LogEntry>,
}

fn push_log(log: &mut VecDeque<LogEntry>, entry: LogEntry) {
    log.push_front(entry);
    log.truncate(LOG_LIMIT);
}

pub struct Jobs {
    ctx: Ctx,
    downloads: Mutex<DownloadQueue>,
    exports: Mutex<ExportQueue>,
    downloads_ready: Notify,
    exports_ready: Notify,
}

impl Jobs {
    /// Creates the queues and spawns their workers.
    pub fn start(ctx: Ctx) -> Arc<Self> {
        let jobs = Arc::new(Self {
            ctx,
            downloads: Mutex::default(),
            exports: Mutex::default(),
            downloads_ready: Notify::new(),
            exports_ready: Notify::new(),
        });
        tokio::spawn(jobs.clone().run_downloads());
        tokio::spawn(jobs.clone().run_exports());
        jobs
    }

    pub fn queue_downloads(&self, track_ids: Vec<i64>, auto_export: Option<AutoExportOptions>) {
        {
            let mut queue = self.downloads.lock().unwrap();
            queue.active = true;
            queue.total += track_ids.len() as u64;
            queue
                .pending
                .extend(track_ids.into_iter().map(|id| (id, auto_export)));
        }
        self.downloads_ready.notify_one();
        self.emit_downloads();
    }

    /// Stops the current batch and clears its progress, like the desktop Stop/Finish buttons.
    pub fn reset_downloads(&self) {
        {
            let mut queue = self.downloads.lock().unwrap();
            let generation = queue.generation + 1;
            *queue = DownloadQueue {
                generation,
                ..Default::default()
            };
        }
        self.emit_downloads();
    }

    pub fn download_snapshot(&self) -> DownloadSnapshot {
        let queue = self.downloads.lock().unwrap();
        let cooldown_remaining = queue
            .cooldown_until
            .map(|until| until.saturating_duration_since(Instant::now()))
            .map(|remaining| remaining.as_secs_f64().ceil() as u64)
            .unwrap_or(0);
        DownloadSnapshot {
            is_downloading: queue.active,
            queued: queue.pending.len(),
            total_count: queue.total,
            downloaded_count: queue.downloaded,
            success_count: queue.succeeded,
            failure_count: queue.failed,
            cooldown_remaining,
            log: queue.log.iter().cloned().collect(),
        }
    }

    fn emit_downloads(&self) {
        let _ = self
            .ctx
            .emit("download-queue-state", self.download_snapshot());
    }

    async fn run_downloads(self: Arc<Self>) {
        loop {
            let next = {
                let mut queue = self.downloads.lock().unwrap();
                match queue.cooldown_until {
                    Some(until) if until > Instant::now() => Err(until - Instant::now()),
                    _ => {
                        queue.cooldown_until = None;
                        Ok(queue
                            .pending
                            .pop_front()
                            .map(|(id, auto_export)| (id, auto_export, queue.generation)))
                    }
                }
            };

            let (track_id, auto_export, generation) = match next {
                // LRCLIB asked us to wait: count down in one-second steps for the UI.
                Err(remaining) => {
                    self.emit_downloads();
                    tokio::time::sleep(remaining.min(Duration::from_secs(1))).await;
                    continue;
                }
                Ok(None) => {
                    self.downloads_ready.notified().await;
                    continue;
                }
                Ok(Some(next)) => next,
            };

            let (entry, retry_after) = self.download_one(track_id, auto_export).await;
            {
                let mut queue = self.downloads.lock().unwrap();
                if queue.generation == generation {
                    if entry.status == "success" {
                        queue.succeeded += 1;
                    } else {
                        queue.failed += 1;
                    }
                    queue.downloaded += 1;
                    push_log(&mut queue.log, entry);
                    if let Some(delay) = retry_after {
                        let until = Instant::now() + delay;
                        if queue.cooldown_until.map_or(true, |current| until > current) {
                            queue.cooldown_until = Some(until);
                        }
                    }
                }
            }
            self.emit_downloads();

            let jitter = fastrand::u64(0..REQUEST_JITTER_MS);
            tokio::time::sleep(Duration::from_millis(REQUEST_SPACING_MS + jitter)).await;
        }
    }

    /// Downloads one track's lyrics. Returns the log entry and, when LRCLIB is rate limiting or
    /// overloaded, how long to pause the queue.
    async fn download_one(
        &self,
        track_id: i64,
        auto_export: Option<AutoExportOptions>,
    ) -> (LogEntry, Option<Duration>) {
        let track = match commands::get_track(&self.ctx, track_id).await {
            Ok(track) => track,
            Err(err) => {
                let entry = LogEntry {
                    status: "failure",
                    title: format!("Track {track_id}"),
                    artist_name: String::new(),
                    message: err,
                };
                return (entry, None);
            }
        };

        let entry = |status, message| LogEntry {
            status,
            title: track.title.clone(),
            artist_name: track.artist_name.clone(),
            message,
        };
        match commands::download_lyrics(&self.ctx, track_id, auto_export).await {
            Ok(message) => (entry("success", message), None),
            Err(err) => {
                let err = serde_json::to_value(err).unwrap_or(Value::Null);
                let kind = err.get("kind").and_then(Value::as_str).unwrap_or_default();
                let retry_after = err
                    .get("retryAfterSecs")
                    .and_then(Value::as_u64)
                    .filter(|secs| *secs > 0)
                    .filter(|_| kind == "rateLimited" || kind == "serverOverloaded")
                    .map(Duration::from_secs);
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Download failed")
                    .to_owned();
                (entry("failure", message), retry_after)
            }
        }
    }

    pub fn queue_exports(&self, track_ids: Vec<i64>, formats: Vec<ExportLyricsFormat>) {
        let formats: Arc<[ExportLyricsFormat]> = formats.into();
        {
            let mut queue = self.exports.lock().unwrap();
            queue.active = true;
            queue.total += track_ids.len() as u64;
            queue
                .pending
                .extend(track_ids.into_iter().map(|id| (id, formats.clone())));
        }
        self.exports_ready.notify_one();
        self.emit_exports();
    }

    pub fn reset_exports(&self) {
        {
            let mut queue = self.exports.lock().unwrap();
            let generation = queue.generation + 1;
            *queue = ExportQueue {
                generation,
                ..Default::default()
            };
        }
        self.emit_exports();
    }

    pub fn export_snapshot(&self) -> ExportSnapshot {
        let queue = self.exports.lock().unwrap();
        ExportSnapshot {
            is_exporting: queue.active,
            queued: queue.pending.len(),
            total_count: queue.total,
            exported_count: queue.exported,
            skipped_count: queue.skipped,
            error_count: queue.errors,
            log: queue.log.iter().cloned().collect(),
        }
    }

    fn emit_exports(&self) {
        let _ = self.ctx.emit("export-queue-state", self.export_snapshot());
    }

    async fn run_exports(self: Arc<Self>) {
        loop {
            let next = {
                let mut queue = self.exports.lock().unwrap();
                let generation = queue.generation;
                queue
                    .pending
                    .pop_front()
                    .map(|(id, formats)| (id, formats, generation))
            };
            let Some((track_id, formats, generation)) = next else {
                self.exports_ready.notified().await;
                continue;
            };

            let entry = self.export_one(track_id, formats.to_vec()).await;
            {
                let mut queue = self.exports.lock().unwrap();
                if queue.generation == generation {
                    match entry.status {
                        "exported" => queue.exported += 1,
                        "skipped" => queue.skipped += 1,
                        _ => queue.errors += 1,
                    }
                    push_log(&mut queue.log, entry);
                }
            }
            self.emit_exports();
            tokio::task::yield_now().await;
        }
    }

    async fn export_one(&self, track_id: i64, formats: Vec<ExportLyricsFormat>) -> LogEntry {
        let track = match commands::get_track(&self.ctx, track_id).await {
            Ok(track) => track,
            Err(err) => {
                return LogEntry {
                    status: "error",
                    title: format!("Track {track_id}"),
                    artist_name: String::new(),
                    message: err,
                }
            }
        };

        let (status, message) =
            match commands::export_track_lyrics(&self.ctx, track_id, formats).await {
                Ok(summary) if summary.errors > 0 => ("error", summary.message),
                Ok(summary) if summary.exported > 0 => ("exported", summary.message),
                Ok(summary) => ("skipped", summary.message),
                Err(err) => ("error", err),
            };
        LogEntry {
            status,
            title: track.title,
            artist_name: track.artist_name,
            message,
        }
    }
}
