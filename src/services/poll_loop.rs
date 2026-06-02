//! Async polling loop — replaces the Slint `Looper` + `Timer` pattern.
//!
//! In Slint, services were polled every 200ms via a `Timer` callback.
//! In GPUI, we use `cx.spawn()` with an async loop and `smol::Timer`.
//!
//! Usage:
//! ```ignore
//! // In your root view's init or a lifecycle hook:
//! cx.spawn(|view, mut cx| async move {
//!     start_poll_loop(view, &mut cx).await;
//! })
//! .detach();
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Poll interval in milliseconds.
pub const POLL_INTERVAL_MS: u64 = 200;

/// Trait for services that tick on each poll cycle.
///
/// Equivalent to the Slint-era `Pollable` trait. Services are held
/// in `Arc<Mutex<>>` for thread safety.
pub trait Pollable {
    fn poll(&mut self);
}

/// Runs the polling loop until cancelled.
///
/// Uses `gpui::Timer` (re-exported from `smol`). Call via `cx.spawn()`
/// in your root view. The loop stops when the spawned future is
/// cancelled (entity dropped).
pub async fn run(services: Arc<Mutex<Vec<Box<dyn Pollable>>>>) {
    loop {
        gpui::Timer::after(Duration::from_millis(POLL_INTERVAL_MS)).await;
        if let Ok(mut svcs) = services.lock() {
            for svc in svcs.iter_mut() {
                svc.poll();
            }
        }
    }
}
