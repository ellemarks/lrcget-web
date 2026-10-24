//! Core of LRCGET, shared by the Tauri desktop app (`src/main.rs`) and the
//! web server (`src/bin/lrcget-server`).

pub mod commands;
pub mod db;
pub mod export;
pub mod library;
pub mod lrclib;
pub mod lyricsfile;
pub mod parser;
pub mod persistent_entities;
#[cfg(feature = "desktop")]
pub mod player;
pub mod scanner;
pub mod state;
pub mod utils;
pub mod word_segmentation;
