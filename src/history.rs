use crate::types::ClipboardItem;

pub struct ClipboardHistory {
    items: Vec<ClipboardItem>,
    max_capacity: usize,
}

impl ClipboardHistory {
    pub fn new(max_capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(max_capacity),
            max_capacity,
        }
    }

    pub fn add(&mut self, item: ClipboardItem) {
        self.items.retain(|e| e.content_hash != item.content_hash);
        self.items.insert(0, item);
        self.items.truncate(self.max_capacity);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn items(&self) -> &[ClipboardItem] {
        &self.items
    }
}
