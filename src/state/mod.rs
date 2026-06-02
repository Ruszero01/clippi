//! GPUI state layer — Entity-based application state.
//!
//! Replaces the Slint-era `Arc<Mutex<>>` + `Rc<VecModel<>>` pattern with
//! GPUI's `Entity<T>` and interior mutability model.

pub mod app;
pub mod clipboard;
pub mod settings;
pub mod sync;
