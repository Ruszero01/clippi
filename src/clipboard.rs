use clipboard_rs::{Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use crate::types::ClipboardItem;

pub enum ClipboardEvent {
    NewContent(ClipboardItem),
}

struct Handler {
    ctx: ClipboardContext,
    sender: Sender<ClipboardEvent>,
    next_id: i64,
}

impl ClipboardHandler for Handler {
    fn on_clipboard_change(&mut self) {
        match self.ctx.get_text() {
            Ok(text) if !text.is_empty() => {
                let item = ClipboardItem::new_text(self.next_id, &text);
                self.next_id += 1;
                let _ = self.sender.send(ClipboardEvent::NewContent(item));
            }
            Ok(_) => {}
            Err(_) => {
                // 非文本内容（图片等）无法读取为文本，忽略即可
            }
        }
    }
}

pub struct ClipboardWatcherHandle {
    shutdown: Option<clipboard_rs::WatcherShutdown>,
    thread: Option<JoinHandle<()>>,
}

impl ClipboardWatcherHandle {
    pub fn stop(&mut self) {
        if let Some(s) = self.shutdown.take() {
            s.stop();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start_watcher(sender: Sender<ClipboardEvent>) -> Result<ClipboardWatcherHandle, Box<dyn std::error::Error + Send + Sync>> {
    let ctx = ClipboardContext::new()?;
    let handler = Handler {
        ctx,
        sender,
        next_id: 1i64,
    };

    let mut watcher = ClipboardWatcherContext::new()?;
    let shutdown = watcher.add_handler(handler).get_shutdown_channel();

    let thread = thread::spawn(move || {
        watcher.start_watch();
    });

    Ok(ClipboardWatcherHandle {
        shutdown: Some(shutdown),
        thread: Some(thread),
    })
}
