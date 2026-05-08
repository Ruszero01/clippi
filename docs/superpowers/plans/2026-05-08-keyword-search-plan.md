# Keyword Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real-time keyword search that combines with existing type filters via AND logic.

**Architecture:** Extend the unified `ClipboardFilters` system with a `keyword` field that adds `LIKE` conditions to existing WHERE clauses. Remove the redundant `text_preview` field, replacing it with `searchable_text` (for search) and UI direct-read of `full_text` (for preview). Search triggers on every keystroke via Slint `TextInput::changed` callback.

**Tech Stack:** Rust + Slint UI + SQLite (rusqlite)

---

## Task 1: Remove `text_preview`, add `searchable_text` to data model

**Files:**
- Modify: `src/core/types.rs`

- [ ] **Step 1: Update `ClipboardItem` struct and constructors**

```rust
// src/core/types.rs

/// A clipboard item
#[derive(Debug, Clone)]
pub struct ClipboardItem {
    pub id: i64,
    pub content_type: ContentType,
    pub full_text: String,
    pub searchable_text: String,
    pub content_hash: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub image_path: String,
}

impl ClipboardItem {
    pub fn new_text(id: i64, text: &str, content_type: ContentType) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let now = Utc::now();
        Self {
            id,
            content_type,
            full_text: text.to_string(),
            searchable_text: text.to_string(),
            content_hash: hasher.finish(),
            created_at: now,
            updated_at: now,
            image_path: String::new(),
        }
    }

    pub fn new_image(id: i64, image_path: &str, hash: u64) -> Self {
        let now = Utc::now();
        Self {
            id,
            content_type: ContentType::Image,
            full_text: image_path.to_string(),
            searchable_text: String::new(),
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: image_path.to_string(),
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/core/types.rs && git commit -m "refactor: replace text_preview with searchable_text in ClipboardItem"
```

---

## Task 2: Update database schema and queries

**Files:**
- Modify: `src/core/db.rs`

- [ ] **Step 1: Update `init_schema` — replace `text_preview` with `searchable_text`, add index**

```rust
fn init_schema(&self) -> SqlResult<()> {
    self.conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS clipboard_items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content_type TEXT NOT NULL DEFAULT 'text',
            full_text TEXT NOT NULL,
            searchable_text TEXT NOT NULL DEFAULT '',
            content_hash INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            image_path TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_hash ON clipboard_items(content_hash);
        CREATE INDEX IF NOT EXISTS idx_updated ON clipboard_items(updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_searchable ON clipboard_items(searchable_text);",
    )
}
```

- [ ] **Step 2: Update `upsert` — write `searchable_text` column**

```rust
pub fn upsert(&self, item: &ClipboardItem) -> SqlResult<()> {
    let changed = self.conn.execute(
        "UPDATE clipboard_items SET updated_at = ?1, image_path = ?3, searchable_text = ?4 WHERE content_hash = ?2",
        params![item.updated_at.to_rfc3339(), item.content_hash as i64, item.image_path, item.searchable_text],
    )?;
    if changed == 0 {
        self.conn.execute(
            "INSERT INTO clipboard_items (content_type, full_text, searchable_text, content_hash, created_at, updated_at, image_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                item.content_type.as_str(),
                item.full_text,
                item.searchable_text,
                item.content_hash as i64,
                item.created_at.to_rfc3339(),
                item.updated_at.to_rfc3339(),
                item.image_path,
            ],
        )?;
    }
    Ok(())
}
```

- [ ] **Step 3: Update `row_to_item` — map `searchable_text` instead of `text_preview`**

```rust
fn row_to_item(row: &rusqlite::Row<'_>) -> SqlResult<ClipboardItem> {
    let ct_str: String = row.get(1)?;
    let created_str: String = row.get(5)?;
    let updated_str: String = row.get(6)?;
    let image_path: String = row.get(7).unwrap_or_default();
    Ok(ClipboardItem {
        id: row.get(0)?,
        content_type: ContentType::from_str(&ct_str),
        full_text: row.get(2)?,
        searchable_text: row.get(3)?,
        content_hash: row.get::<_, i64>(4)? as u64,
        created_at: created_str.parse().unwrap_or_default(),
        updated_at: updated_str.parse().unwrap_or_default(),
        image_path,
    })
}
```

- [ ] **Step 4: Update all SELECT queries — replace `text_preview` with `searchable_text`**

Change column name in `load_filtered()`, `get_by_id()`, `get_by_hash()`:

```rust
// In load_filtered():
"SELECT id, content_type, full_text, searchable_text, content_hash, created_at, updated_at, image_path
 FROM clipboard_items {} ORDER BY {} DESC LIMIT ?"

// In get_by_id():
"SELECT id, content_type, full_text, searchable_text, content_hash, created_at, updated_at, image_path
 FROM clipboard_items WHERE id = ?1"

// In get_by_hash():
"SELECT id, content_type, full_text, searchable_text, content_hash, created_at, updated_at, image_path
 FROM clipboard_items WHERE content_hash = ?1"
```

- [ ] **Step 5: Build check**

Run: `cargo check`
Expected: Compile errors only if `text_preview` is still referenced elsewhere (service layer, which we'll fix in Task 5)

- [ ] **Step 6: Commit**

```bash
git add src/core/db.rs && git commit -m "refactor: replace text_preview with searchable_text in DB schema and queries"
```

---

## Task 3: Extend `ClipboardFilters` with keyword field

**Files:**
- Modify: `src/core/filters.rs`

- [ ] **Step 1: Add `keyword` field and update all methods**

```rust
// src/core/filters.rs

/// Unified filter state for clipboard queries.
///
/// Future dimensions can be added as new fields:
/// - `tags: Vec<String>` for custom tags
/// - `favorite: bool` for starred items
///
/// All active dimensions combine with AND logic.
#[derive(Debug, Clone, Default)]
pub struct ClipboardFilters {
    /// Content type filter: empty = all types, non-empty = any of these types
    type_filters: Vec<String>,
    /// Keyword search: None = no filter, Some = LIKE %keyword% on searchable_text
    keyword: Option<String>,
}

impl ClipboardFilters {
    /// Toggle a content type filter on/off
    pub fn toggle_type(&mut self, type_name: &str) {
        if let Some(pos) = self.type_filters.iter().position(|t| t == type_name) {
            self.type_filters.remove(pos);
        } else {
            self.type_filters.push(type_name.to_string());
        }
    }

    /// Set keyword search filter
    pub fn set_keyword(&mut self, keyword: &str) {
        self.keyword = if keyword.is_empty() {
            None
        } else {
            Some(keyword.to_string())
        };
    }

    /// Clear all filters across all dimensions
    pub fn clear_all(&mut self) {
        self.type_filters.clear();
        self.keyword = None;
    }

    /// Returns true when no filters are active
    pub fn is_empty(&self) -> bool {
        self.type_filters.is_empty() && self.keyword.is_none()
    }

    /// Check if a specific type filter is active
    pub fn is_type_active(&self, type_name: &str) -> bool {
        self.type_filters.iter().any(|t| t == type_name)
    }

    /// Check if keyword search is active
    pub fn has_keyword(&self) -> bool {
        self.keyword.is_some()
    }

    /// Check if an in-memory item matches all active filters (AND logic).
    /// Used during poll() for real-time filtering of incoming items.
    pub fn matches_item(&self, item: &ClipboardItem) -> bool {
        // Type filter dimension
        if !self.type_filters.is_empty() {
            let type_str = item.content_type.as_str();
            if !self.type_filters.iter().any(|t| t.as_str() == type_str) {
                return false;
            }
        }
        // Keyword filter: match against full_text for text types, skip images
        if let Some(ref kw) = self.keyword {
            match item.content_type {
                crate::core::types::ContentType::Image => return false,
                _ => {
                    if !item.full_text.to_lowercase().contains(&kw.to_lowercase()) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Build SQL WHERE clause and params for database queries.
    /// Returns (sql_fragment, params) where sql_fragment may be empty string if no filters.
    pub fn db_where(&self) -> (String, Vec<rusqlite::types::Value>) {
        let mut conditions = Vec::new();
        let mut params = Vec::new();

        // Type filter
        if !self.type_filters.is_empty() {
            let placeholders: Vec<&str> = self.type_filters.iter().map(|_| "?").collect();
            conditions.push(format!(
                "content_type IN ({})",
                placeholders.join(", ")
            ));
            for t in &self.type_filters {
                params.push(t.clone().into());
            }
        }

        // Keyword filter
        if let Some(ref kw) = self.keyword {
            conditions.push("searchable_text LIKE ?".to_string());
            params.push(format!("%{}%", kw).into());
        }

        if conditions.is_empty() {
            (String::new(), params)
        } else {
            (format!("WHERE {}", conditions.join(" AND ")), params)
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/core/filters.rs && git commit -m "feat: add keyword field to ClipboardFilters for search"
```

---

## Task 4: Add search UI to `ClipboardList.slint`

**Files:**
- Modify: `ui/ClipboardList.slint`

- [ ] **Step 1: Insert search box before filter bar (inside VerticalLayout, above the filter bar HorizontalLayout)**

Insert after `spacing: 4px;` (line 45) and before `// ========== 筛选按钮栏 ==========` (line 47):

```slint
        // ========== 搜索框 ==========
        Rectangle {
            height: 32px;
            border-radius: 8px;
            background: dark-mode ? #1e2038 : #f0f1f7;
            padding-left: 8px;
            padding-right: 8px;

            HorizontalLayout {
                spacing: 6px;
                alignment: center;

                Text {
                    text: "\u{e613}";
                    font-family: "iconfont";
                    font-size: 14px;
                    color: text-3;
                    vertical-alignment: center;
                }

                search-input := TextInput {
                    font-size: 12px;
                    color: text-1;
                    placeholder-text: "搜索剪贴板内容...";
                    placeholder-color: text-3;
                    background: transparent;
                    border-width: 0px;
                    vertical-alignment: center;
                    changed(text) => {
                        root.search-keyword(text);
                    }
                }
            }
        }
```

- [ ] **Step 2: Add `search-keyword` callback to the component's callback list**

After line 24 (`callback clear-filters();`):

```slint
    callback search-keyword(string);
```

- [ ] **Step 3: Commit**

```bash
git add ui/ClipboardList.slint && git commit -m "feat: add search TextInput to ClipboardList UI"
```

---

## Task 5: Add `search-keyword` callback to `app.slint`

**Files:**
- Modify: `ui/app.slint`

- [ ] **Step 1: Add callback declaration**

After line 66 (`callback clear-filters();`):

```slint
    callback search-keyword(string);
```

- [ ] **Step 2: Propagate callback from ClipboardList to App**

After line 312 (`clear-filters => { root.clear-filters(); }`):

```slint
                    search-keyword(keyword) => { root.search-keyword(keyword); }
```

- [ ] **Step 3: Commit**

```bash
git add ui/app.slint && git commit -m "feat: add search-keyword callback to App"
```

---

## Task 6: Update clipboard service — `set_keyword`, `clear_keyword`, `item_to_entry`

**Files:**
- Modify: `src/services/clipboard.rs`

- [ ] **Step 1: Add `set_keyword` and `clear_keyword` methods to `ClipboardService`**

After `clear_filters()` method (line 53-59), add:

```rust
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

    /// Check if keyword search is active
    pub fn has_keyword(&self) -> bool {
        self.filters.has_keyword()
    }
```

- [ ] **Step 2: Update `item_to_entry` — read `full_text` directly for preview**

```rust
fn item_to_entry(item: &crate::core::types::ClipboardItem) -> ClipboardEntry {
    let thumbnail = if !item.image_path.is_empty() {
        Image::load_from_path(std::path::Path::new(&item.image_path)).unwrap_or_default()
    } else {
        Image::default()
    };
    ClipboardEntry {
        id: item.id as i32,
        preview: SharedString::from(item.full_text.clone()),
        content_type: SharedString::from(item.content_type.as_str()),
        time_label: SharedString::from(format_relative_time(&item.updated_at)),
        image_path: SharedString::from(item.image_path.clone()),
        thumbnail,
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add src/services/clipboard.rs && git commit -m "feat: add set_keyword/clear_keyword and use full_text for preview"
```

---

## Task 7: Wire `on_search_keyword` callback in `app.rs`

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Add search callback binding**

After the `on_clear_filters` block (line 347-359), add:

```rust
        // Search callback
        let looper_for_search = Arc::clone(&looper);
        slint_app.on_search_keyword(move |keyword: SharedString| {
            let kw = keyword.to_string();
            let _ = looper_for_search.try_with_clipboard_service(|cs| {
                if kw.is_empty() {
                    cs.clear_keyword();
                } else {
                    cs.set_keyword(&kw);
                }
            });
        });
```

- [ ] **Step 2: Commit**

```bash
git add src/app.rs && git commit -m "feat: wire on_search_keyword callback to clipboard service"
```

---

## Task 8: Build and verify

**Files:**
- None (verification only)

- [ ] **Step 1: Build check**

```bash
cargo check
```
Expected: Clean compile, no errors.

- [ ] **Step 2: Cargo build**

```bash
cargo build
```
Expected: Successful build.

- [ ] **Step 3: Manual smoke test plan**

1. Run `cargo run`
2. Copy some text → verify it appears in the list
3. Type in search box → verify list filters in real-time
4. Clear search → verify all items show again
5. Combine search + type filter → verify AND logic works
6. Search for text that doesn't exist → verify empty state shows
