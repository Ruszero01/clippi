//! Clipboard service - handles clipboard business logic

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::paths::images_dir;
use crate::core::types::format_relative_time;
use crate::looper::Pollable;
use crate::platform::clipboard::ClipboardShared;
use crate::platform::favicon;
use crate::platform::file_icon;
use crate::App;
use crate::ClipboardEntry;
use base64::Engine;
use slint::{Image, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;

/// Maximum items to keep in memory
const MAX_ITEMS: usize = 100;

#[derive(Clone)]
pub struct ClipboardService {
    shared: ClipboardShared,
    db: Arc<Mutex<Database>>,
    app: slint::Weak<App>,
    model: Rc<VecModel<ClipboardEntry>>,
    sort_by_created: bool,
    copy_as_plain_text: bool,
    filters: ClipboardFilters,
    selected_ids: Vec<i32>,
    anchor_id: i32,
}

impl ClipboardService {
    pub fn new(
        shared: ClipboardShared,
        db: Arc<Mutex<Database>>,
        app: slint::Weak<App>,
    ) -> Self {
        let model: Rc<VecModel<ClipboardEntry>> = Rc::new(VecModel::default());
        let service = Self {
            shared,
            db,
            app,
            model: model.clone(),
            sort_by_created: false,
            copy_as_plain_text: false,
            filters: ClipboardFilters::default(),
            selected_ids: Vec::new(),
            anchor_id: -1,
        };
        if let Some(app) = service.app.upgrade() {
            app.set_clipboard_items(ModelRc::from(model));
            app.set_item_count(0);
        }
        service
    }

    /// Toggle a content type filter and reload from database
    pub fn toggle_filter_and_refresh(&mut self, filter_type: &str) {
        self.filters.toggle_type(filter_type);
        self.refresh_with_current_filter();
    }

    /// Toggle the combined "file" filter (image + file types) atomically.
    /// Avoids double-refresh by modifying both type filters before reloading.
    pub fn toggle_file_filter_and_refresh(&mut self) {
        // Toggle both "image" and "file" type_filters together
        let file_active = self.filters.is_type_active("file");
        let image_active = self.filters.is_type_active("image");
        if file_active && image_active {
            // Both active → deactivate both
            self.filters.toggle_type("file");
            self.filters.toggle_type("image");
        } else {
            // At least one inactive → activate both
            if !file_active { self.filters.toggle_type("file"); }
            if !image_active { self.filters.toggle_type("image"); }
        }
        self.refresh_with_current_filter();
    }

    /// Toggle the combined "链接" filter (link + path types) atomically.
    pub fn toggle_link_filter_and_refresh(&mut self) {
        let link_active = self.filters.is_type_active("link");
        let path_active = self.filters.is_type_active("path");
        if link_active && path_active {
            self.filters.toggle_type("link");
            self.filters.toggle_type("path");
        } else {
            if !link_active { self.filters.toggle_type("link"); }
            if !path_active { self.filters.toggle_type("path"); }
        }
        self.refresh_with_current_filter();
    }

    /// Toggle favorites filter and reload from database
    pub fn toggle_favorites_filter_and_refresh(&mut self) {
        self.filters.toggle_favorites_only();
        self.refresh_with_current_filter();
    }

    /// Clear all filters and reload
    pub fn clear_filters(&mut self) {
        self.filters.clear_all();
        self.refresh_with_current_filter();
    }

    /// Set keyword search and reload from database
    pub fn set_keyword(&mut self, keyword: &str) {
        self.filters.set_keyword(keyword);
        self.refresh_with_current_filter();
    }

    /// Clear keyword search and reload
    pub fn clear_keyword(&mut self) {
        self.filters.set_keyword("");
        self.refresh_with_current_filter();
    }

    fn refresh_with_current_filter(&mut self) {
        self.model.clear();
        // 筛选变化时清除选中状态
        self.selected_ids.clear();
        self.anchor_id = -1;
        let order_by = if self.sort_by_created { "created_at" } else { "updated_at" };
        let items = self.db.lock().expect("db lock poisoned")
            .load_filtered(&self.filters, MAX_ITEMS, order_by)
            .unwrap_or_default();
        for item in items {
            self.model.push(item_to_entry(&item));
        }
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
            app.set_selected_count(0);
        }
    }

    /// Set sort mode and refresh (full reload)
    pub fn set_sort_and_refresh(&mut self, sort_by_created: bool) {
        self.sort_by_created = sort_by_created;
        self.refresh_with_current_filter();
    }

    /// Toggle copy-as-plain-text mode (no reload needed — applies to incoming items)
    pub fn set_copy_as_plain_text(&mut self, enabled: bool) {
        self.copy_as_plain_text = enabled;
    }

    /// Load initial items from database (model starts empty; delegate to unified refresh)
    pub fn load_initial(&mut self) {
        self.refresh_with_current_filter();
    }

    /// Refresh a single row by id (e.g., after copying to update timestamp)
    pub fn refresh_row(&self, id: i32) {
        if let Ok(db) = self.db.lock() {
            if let Ok(Some(item)) = db.get_by_id(id as i64) {
                for i in 0..self.model.row_count() {
                    if let Some(entry) = self.model.row_data(i) {
                        if entry.id == id {
                            self.model.set_row_data(i, item_to_entry(&item));
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Check if a filter type is active
    pub fn is_filter_active(&self, filter_type: &str) -> bool {
        self.filters.is_type_active(filter_type)
    }

    /// Check if favorites filter is active
    pub fn is_favorites_filter_active(&self) -> bool {
        self.filters.is_favorites_active()
    }

    /// Toggle favorite status for an item and refresh that row
    pub fn toggle_favorite(&mut self, id: i32) {
        let needs_full_refresh = self.filters.is_favorites_active();
        {
            if let Ok(db) = self.db.lock() {
                let _ = db.toggle_favorite(id as i64);
            }
        }
        if needs_full_refresh {
            self.refresh_with_current_filter();
        } else {
            self.refresh_row(id);
        }
    }

    /// Delete an item from database and remove from model
    pub fn delete_item(&mut self, id: i32) {
        if let Ok(db) = self.db.lock() {
            let _ = db.delete_item(id as i64);
        }
        for i in 0..self.model.row_count() {
            if let Some(entry) = self.model.row_data(i) {
                if entry.id == id {
                    self.model.remove(i);
                    break;
                }
            }
        }
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
        }
    }

    /// Update note for an item and refresh that row
    pub fn update_note(&mut self, id: i32, note: &str) {
        if let Ok(db) = self.db.lock() {
            let _ = db.update_note(id as i64, note);
        }
        self.refresh_row(id);
    }

    /// Update content for an item, recompute hash, and refresh that row
    pub fn update_content(&mut self, id: i32, text: &str, content_type: &str) {
        if let Ok(db) = self.db.lock() {
            let _ = db.update_content(id as i64, text, content_type);
        }
        self.refresh_row(id);
    }

    /// Get reference to shared buffer for platform layer
    pub fn shared(&self) -> &ClipboardShared {
        &self.shared
    }

    // ── Selection management ──

    /// Select a single item; deselect all others
    pub fn select_single(&mut self, id: i32) {
        self.selected_ids.clear();
        self.selected_ids.push(id);
        self.anchor_id = id;
        self.sync_selection_to_model();
    }

    /// Toggle selection of an item (Ctrl+click)
    pub fn toggle_selection(&mut self, id: i32) {
        if let Some(pos) = self.selected_ids.iter().position(|&x| x == id) {
            self.selected_ids.remove(pos);
        } else {
            self.selected_ids.push(id);
            self.anchor_id = id;
        }
        self.sync_selection_to_model();
    }

    /// Range select from anchor to given id (Shift+click)
    pub fn range_select(&mut self, id: i32) {
        // Fall back to single select if no anchor exists
        if self.anchor_id < 0 || self.selected_ids.is_empty() {
            self.select_single(id);
            return;
        }
        let mut anchor_idx: Option<usize> = None;
        let mut clicked_idx: Option<usize> = None;
        for i in 0..self.model.row_count() {
            if let Some(entry) = self.model.row_data(i) {
                if entry.id == self.anchor_id {
                    anchor_idx = Some(i);
                }
                if entry.id == id {
                    clicked_idx = Some(i);
                }
            }
        }

        if let (Some(a), Some(c)) = (anchor_idx, clicked_idx) {
            self.selected_ids.clear();
            if a <= c {
                for i in a..=c {
                    if let Some(entry) = self.model.row_data(i) {
                        self.selected_ids.push(entry.id);
                    }
                }
            } else {
                for i in (c..=a).rev() {
                    if let Some(entry) = self.model.row_data(i) {
                        self.selected_ids.push(entry.id);
                    }
                }
            }
        }
        self.sync_selection_to_model();
    }

    /// Clear all selection
    pub fn clear_selection(&mut self) {
        self.selected_ids.clear();
        self.anchor_id = -1;
        self.sync_selection_to_model();
    }

    /// Sync in-memory selection state to model `selected` / `selection_order` fields
    fn sync_selection_to_model(&self) {
        // Set count BEFORE model updates so Slint bindings use the correct value
        // when re-evaluating visibility of selection-dependent elements (e.g. badge).
        if let Some(app) = self.app.upgrade() {
            app.set_selected_count(self.selected_ids.len() as i32);
        }
        for i in 0..self.model.row_count() {
            if let Some(mut entry) = self.model.row_data(i) {
                let pos = self.selected_ids.iter().position(|&x| x == entry.id);
                entry.selected = pos.is_some();
                entry.selection_order = pos.map(|p| p as i32).unwrap_or(-1);
                self.model.set_row_data(i, entry);
            }
        }
    }

    /// Get selected items in selection order
    pub fn get_selected_items(&self) -> Vec<crate::core::types::ClipboardItem> {
        let db = self.db.lock().expect("db lock poisoned");
        self.selected_ids
            .iter()
            .filter_map(|&id| db.get_by_id(id as i64).ok().flatten())
            .collect()
    }

    /// Batch toggle favorite on all selected items
    pub fn batch_toggle_favorite(&mut self) {
        let needs_full_refresh = self.filters.is_favorites_active();
        {
            let db = self.db.lock().expect("db lock poisoned");
            for &id in &self.selected_ids {
                let _ = db.toggle_favorite(id as i64);
            }
        }
        if needs_full_refresh {
            self.refresh_with_current_filter();
            self.clear_selection();
        } else {
            for &id in &self.selected_ids {
                self.refresh_row(id);
            }
            // Re-sync selection after refresh
            self.sync_selection_to_model();
        }
    }

    /// Batch delete all selected items
    pub fn batch_delete(&mut self) {
        {
            let db = self.db.lock().expect("db lock poisoned");
            for &id in &self.selected_ids {
                let _ = db.delete_item(id as i64);
            }
        }
        // Remove from model (iterate in reverse to maintain indices)
        let ids = self.selected_ids.clone();
        self.selected_ids.clear();
        self.anchor_id = -1;
        for id in ids {
            for i in 0..self.model.row_count() {
                if let Some(entry) = self.model.row_data(i) {
                    if entry.id == id {
                        self.model.remove(i);
                        break;
                    }
                }
            }
        }
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
            app.set_selected_count(0);
        }
    }
}

impl Pollable for ClipboardService {
    fn poll(&mut self) {
        // Check if batch paste completed and wants selection cleared
        if self.shared.clear_selection_requested.swap(false, Ordering::SeqCst) {
            self.clear_selection();
        }

        let pending = {
            let mut p = self.shared.pending.lock().expect("clipboard pending lock poisoned");
            p.drain(..).collect::<Vec<_>>()
        };

        if pending.is_empty() {
            return;
        }

        if let Ok(db) = self.db.lock() {
            for mut item in pending {
                // Convert RichText → PlainText when copy-as-plain-text is enabled
                if self.copy_as_plain_text && item.content_type == crate::core::types::ContentType::RichText {
                    item.content_type = crate::core::types::ContentType::PlainText;
                    item.rich_data = String::new();
                }

                if let Err(e) = db.upsert(&item) {
                    eprintln!("[ERROR] ClipboardService: upsert error: {:?}", e);
                    continue;
                }

                // Check if item matches current filters
                if !self.filters.matches_item(&item) {
                    continue;
                }

                if let Ok(Some(existing)) = db.get_by_hash(item.content_hash) {
                    if self.sort_by_created {
                        // created_at doesn't change on dedup, so item should stay
                        // in place. Just update the row with fresh timestamp/label.
                        if let Some(idx) = (0..self.model.row_count()).position(|i| {
                            self.model.row_data(i).map(|e| e.id as i64 == existing.id).unwrap_or(false)
                        }) {
                            self.model.set_row_data(idx, item_to_entry(&existing));
                        }
                    } else {
                        let existing_idx = (0..self.model.row_count()).position(|i| {
                            self.model.row_data(i).map(|e| e.id as i64 == existing.id).unwrap_or(false)
                        });
                        if let Some(idx) = existing_idx {
                            self.model.remove(idx);
                        }
                        self.model.insert(0, item_to_entry(&existing));
                    }
                } else {
                    self.model.insert(0, item_to_entry(&item));
                }
            }
        }

        while self.model.row_count() > MAX_ITEMS {
            self.model.remove(self.model.row_count() - 1);
        }

        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
        }
    }
}

fn item_to_entry(item: &crate::core::types::ClipboardItem) -> ClipboardEntry {
    let thumbnail = if !item.image_path.is_empty() {
        Image::load_from_path(std::path::Path::new(&item.image_path)).unwrap_or_default()
    } else {
        Image::default()
    };
    let (img_w, img_h) = if !item.image_path.is_empty() {
        image::image_dimensions(&item.image_path).unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    // Decode source app icon from base64 and save to disk for Slint Image loading.
    // Use sanitized app name as filename so each app caches only one icon.
    let (source_icon_path, source_icon_image) = if !item.source_app_icon.is_empty() {
        let icon_dir = images_dir().join("icons");
        let _ = std::fs::create_dir_all(&icon_dir);
        let safe_name: String = item.source_app_name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let icon_path = if safe_name.is_empty() {
            icon_dir.join(format!("{:016x}.png", item.content_hash))
        } else {
            icon_dir.join(format!("{}.png", safe_name))
        };
        let path_str = icon_path.to_string_lossy().to_string();

        // Write decoded PNG only if not cached
        if !icon_path.exists() {
            if let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(&item.source_app_icon) {
                let _ = std::fs::write(&icon_path, png_bytes);
            }
        }

        let img = Image::load_from_path(std::path::Path::new(&icon_path)).unwrap_or_default();
        (path_str, img)
    } else {
        (String::new(), Image::default())
    };

    // Resolve color swatch for Color type items
    let color_swatch = if item.content_type == crate::core::types::ContentType::Color {
        crate::core::color::detect_color(&item.full_text)
            .map(|c| slint::Color::from_rgb_u8(c.r, c.g, c.b))
            .unwrap_or_default()
    } else {
        slint::Color::default()
    };

    // File type fields
    let (file_count, file_icon_text, file_name_1, file_name_2, file_name_3, file_overflow) =
        if item.content_type == crate::core::types::ContentType::File && !item.file_data.is_empty() {
            let fd = crate::core::types::FileData::from_json(&item.file_data);
            let count = fd.files.len() as i32;
            let icon_text = if count == 1 {
                if fd.files[0].is_dir {
                    "文件夹".to_string()
                } else {
                    crate::core::types::get_extension_label(&fd.files[0].name)
                }
            } else {
                format!("{}个文件", count)
            };
            let n1 = fd.files.first().map(|f| f.name.clone()).unwrap_or_default();
            let n2 = fd.files.get(1).map(|f| f.name.clone()).unwrap_or_default();
            let n3 = fd.files.get(2).map(|f| f.name.clone()).unwrap_or_default();
            let overflow = if count > 3 { count - 3 } else { 0 };
            (count, icon_text, n1, n2, n3, overflow)
        } else {
            (0, String::new(), String::new(), String::new(), String::new(), 0)
        };

    // Extract OS file icon for the first file.
    // Executables (exe/dll/msi/scr/cpl) embed unique icons → cache by filename.
    // Other files derive icons from associated program → cache by extension.
    let (file_icon_path, file_icon_image) = if item.content_type == crate::core::types::ContentType::File && !item.file_data.is_empty() {
        let fd = crate::core::types::FileData::from_json(&item.file_data);
        if let Some(first_file) = fd.files.first() {
            let cache_name = if first_file.is_dir {
                "ext_folder".to_string()
            } else {
                let ext = std::path::Path::new(&first_file.name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_default();
                if matches!(ext.as_str(), "exe" | "dll" | "msi" | "scr" | "cpl") {
                    let stem = std::path::Path::new(&first_file.name)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_lowercase())
                        .unwrap_or_else(|| "unknown".to_string());
                    format!("app_{}", stem)
                } else if ext.is_empty() {
                    "ext_file".to_string()
                } else {
                    format!("ext_{}", ext)
                }
            };
            if let Some(icon_base64) = file_icon::extract_file_icon_base64(&first_file.path) {
                let icon_dir = images_dir().join("icons");
                let _ = std::fs::create_dir_all(&icon_dir);
                let icon_path = icon_dir.join(format!("{}.png", cache_name));
                let path_str = icon_path.to_string_lossy().to_string();
                if !icon_path.exists() {
                    if let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(&icon_base64) {
                        let _ = std::fs::write(&icon_path, png_bytes);
                    }
                }
                let img = Image::load_from_path(std::path::Path::new(&path_str)).unwrap_or_default();
                (path_str, img)
            } else {
                (String::new(), Image::default())
            }
        } else {
            (String::new(), Image::default())
        }
    } else {
        (String::new(), Image::default())
    };

    // Link/URL preview fields
    let (link_domain, link_path, favicon_path, favicon_image, folder_icon_path, folder_icon_image) =
        match item.content_type {
            crate::core::types::ContentType::Link => {
                let domain = crate::core::types::url_domain(&item.full_text);
                let path = crate::core::types::url_path(&item.full_text);
                // Try cached favicon
                let cache_path = favicon::favicon_cache_path(&domain);
                let cp = std::path::PathBuf::from(&cache_path);
                let (fav_path_str, fav_img) = if cp.exists() {
                    let img = Image::load_from_path(std::path::Path::new(&cache_path)).unwrap_or_default();
                    (cache_path, img)
                } else {
                    (String::new(), Image::default())
                };
                (SharedString::from(domain), SharedString::from(path),
                 SharedString::from(fav_path_str), fav_img,
                 SharedString::default(), Image::default())
            }
            crate::core::types::ContentType::Path => {
                // Extract platform folder icon, cached once
                let icon_dir = images_dir().join("icons");
                let _ = std::fs::create_dir_all(&icon_dir);
                let icon_path = icon_dir.join("path_folder.png");
                if !icon_path.exists() {
                    let sample_dir = std::path::Path::new(&item.full_text)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| item.full_text.clone());
                    if let Some(b64) = file_icon::extract_file_icon_base64(&sample_dir) {
                        if let Ok(png) = base64::engine::general_purpose::STANDARD.decode(&b64) {
                            let _ = std::fs::write(&icon_path, png);
                        }
                    }
                }
                let (fol_path_str, fol_img) = if icon_path.exists() {
                    let img = Image::load_from_path(std::path::Path::new(&icon_path)).unwrap_or_default();
                    (icon_path.to_string_lossy().to_string(), img)
                } else {
                    (String::new(), Image::default())
                };
                (SharedString::default(), SharedString::default(),
                 SharedString::default(), Image::default(),
                 SharedString::from(fol_path_str), fol_img)
            }
            _ => (SharedString::default(), SharedString::default(),
                  SharedString::default(), Image::default(),
                  SharedString::default(), Image::default())
        };

    ClipboardEntry {
        id: item.id as i32,
        preview: SharedString::from(item.full_text.clone()),
        content_type: SharedString::from(item.content_type.as_str()),
        time_label: SharedString::from(format_relative_time(&item.updated_at)),
        image_path: SharedString::from(item.image_path.clone()),
        thumbnail,
        preview_length: item.full_text.len() as i32,
        image_width: img_w as i32,
        image_height: img_h as i32,
        is_favorite: item.is_favorite,
        note: SharedString::from(item.note.clone()),
        source_app_name: SharedString::from(item.source_app_name.clone()),
        source_app_icon_path: SharedString::from(source_icon_path),
        source_app_icon_image: source_icon_image,
        color_swatch,
        link_domain,
        link_path,
        favicon_path,
        favicon_image,
        folder_icon_path,
        folder_icon_image,
        selected: false,
        selection_order: -1,
        file_count,
        file_icon_text: SharedString::from(file_icon_text),
        file_name_1: SharedString::from(file_name_1),
        file_name_2: SharedString::from(file_name_2),
        file_name_3: SharedString::from(file_name_3),
        file_overflow,
        file_icon_path: SharedString::from(file_icon_path),
        file_icon_image,
    }
}
