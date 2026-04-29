//! Core layer - pure Rust, no platform code

pub mod db;
pub mod frontend;
pub mod types;

pub use db::Database;
pub use frontend::Frontend;
pub use types::{ClipboardItem, ContentType};
