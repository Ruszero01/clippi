//! --- Core layer - pure Rust, no platform code ---

pub mod backup;
pub mod cache_cleanup;
pub mod color;
pub mod config_sync;
pub mod data_operation;
pub mod db;
pub mod emoji;
pub mod filters;
pub mod find_replace;
pub mod frontend;
pub mod html_text;
pub mod i18n;
pub mod i18n_keys;
pub mod links;
pub mod migration;
pub mod ocr;
pub mod paste_plain;
pub mod paths;
pub mod qr;
pub mod reorder;
pub mod rtf_text;
pub mod search;
pub mod secret;
pub mod settings;
pub mod symbols;
pub mod sync;
pub mod sync_scope;
pub mod transfer_types;
pub mod types;

pub mod kaomoji;
