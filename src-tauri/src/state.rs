use rusqlite::Connection;
use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotifyType {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "warning")]
    Warning,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notify {
    pub message: String,
    #[serde(rename = "type")]
    pub notify_type: NotifyType,
}

pub struct AppState {
    pub db: std::sync::Mutex<Option<Connection>>,
    pub queued_notifications: std::sync::Mutex<Vec<Notify>>,
}

impl AppState {
    pub fn new(db: Connection) -> Self {
        Self {
            db: std::sync::Mutex::new(Some(db)),
            queued_notifications: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn notify(&self, message: String, notify_type: NotifyType) {
        self.queued_notifications
            .lock()
            .unwrap()
            .push(Notify { message, notify_type });
    }
}

pub trait ServiceAccess {
    fn db<F, TResult>(&self, operation: F) -> TResult
    where
        F: FnOnce(&Connection) -> TResult;

    fn db_mut<F, TResult>(&self, operation: F) -> TResult
    where
        F: FnOnce(&mut Connection) -> TResult;
}

impl ServiceAccess for AppState {
    fn db<F, TResult>(&self, operation: F) -> TResult
    where
        F: FnOnce(&Connection) -> TResult,
    {
        let db_connection_guard = self.db.lock().unwrap();
        let db = db_connection_guard.as_ref().unwrap();

        operation(db)
    }

    fn db_mut<F, TResult>(&self, operation: F) -> TResult
    where
        F: FnOnce(&mut Connection) -> TResult,
    {
        let mut db_connection_guard = self.db.lock().unwrap();
        let db = db_connection_guard.as_mut().unwrap();

        operation(db)
    }
}
