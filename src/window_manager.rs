use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 统一窗口显示/隐藏管理
/// 在 show() 后设置短暂 suppress 期间，防止自动隐藏立即触发
pub struct WindowManager {
    suppress_until: Arc<AtomicU64>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            suppress_until: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 显示窗口并设置 suppress 期间（200ms）
    pub fn show(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // suppress 200ms
        self.suppress_until.store(now + 200, Ordering::SeqCst);
    }

    /// 检查当前是否处于 suppress 期间
    pub fn is_suppressed(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let until = self.suppress_until.load(Ordering::SeqCst);
        now < until
    }
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WindowManager {
    fn clone(&self) -> Self {
        Self {
            suppress_until: self.suppress_until.clone(),
        }
    }
}
