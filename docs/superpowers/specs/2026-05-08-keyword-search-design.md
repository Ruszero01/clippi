# 关键词搜索功能设计

日期: 2026-05-08 | 状态: 待实现

## 概述

在现有分类筛选系统基础上，新增关键词搜索框，支持实时输入过滤。搜索与已有类型筛选通过 AND 逻辑组合，未来可继续扩展标签、收藏等维度。预留了图片 OCR 搜索的可扩展架构。

## 数据库变更

### Schema 变更

`clipboard_items` 表修改：
- **新增** `searchable_text TEXT NOT NULL DEFAULT ''` — 始终是纯文本，仅用于搜索匹配
- **移除** `text_preview` 字段 — 预览改由 UI 从 `full_text` 实时裁剪
- **新增索引** `idx_searchable` 在 `searchable_text` 上，加速 LIKE 查询

### 各内容类型的 searchable_text 填充策略

| 类型 | `full_text` | `searchable_text` |
|------|------------|-------------------|
| 纯文本 | 原文 | 同 full_text |
| 链接 | 原文 URL | 同 full_text |
| 富文本 | 原始 HTML | 去除 HTML 标签的纯文本 |
| 图片 | 图片路径 | 空字符串（未来存 OCR 内容） |

### 迁移方式

直接重建表（开发阶段），修改 `db.rs` 中 `CREATE TABLE` 语句。

## 过滤器扩展

`ClipboardFilters` 新增字段：

```rust
pub keyword: Option<String>,  // None = 不过滤关键词
```

### 方法变更

- `clear_all()` — 同时设置 `keyword = None`
- `is_empty()` — `keyword.is_none()` 也纳入判断
- `matches_item(&ClipboardItem)` — 追加：若 keyword 有值，对 text/rich_text/link 类型用 `full_text` 做 `contains` 匹配；对 image 类型恒不匹配（无 searchable_text，等 OCR 后扩展）
- `db_where()` — 追加 `AND searchable_text LIKE ?` 条件，参数为 `%keyword%`

## 类型层

`ClipboardItem` 结构体：
- 移除 `text_preview` 字段
- 新增 `searchable_text: String` 字段
- `new_text()` / `new_image()` 构造函数同步调整

## 服务层

`ClipboardService` 变更：
- `item_to_entry()`：`preview` 改为从 `full_text` 取值（Slint 的 `overflow: elide` 处理显示裁剪）

- `set_keyword(&mut self, keyword: &str)` — 设置关键词到 `filters.keyword`，调用 `refresh_with_current_filter()`
- `clear_keyword(&mut self)` — 清空关键词，调用 `refresh_with_current_filter()`
- `upsert()` 时传入 `searchable_text` 写入数据库

## UI

### 搜索框 (`ClipboardList.slint`)

- 位置：筛选栏正上方
- 组件：`TextInput`，圆角 8px，左侧搜索图标
- Placeholder：搜索剪贴板内容...
- 触发：`changed(text)` 回调，每次文本变更触发

### 防抖

搜索输入通过 Slint Timer 实现 300ms 防抖。

## 数据流

```
用户输入关键词
  → Slint changed(text) 回调
  → 320ms Timer 防抖
  → app.rs on_search_keyword(keyword)
  → ClipboardService.set_keyword() / clear_keyword()
  → filters.keyword = Some(keyword)
  → refresh_with_current_filter()
  → db_where() 追加 AND searchable_text LIKE '%keyword%'
  → load_filtered() 返回匹配条目
  → UI 刷新列表
```

## 未来扩展预留

| 扩展功能 | 如何接入 |
|---------|---------|
| 图片 OCR | OCR 文本写入 `searchable_text`，搜索自动匹配图片 |
| 标签筛选 | `ClipboardFilters` 新增 `tag_filters` 字段，`db_where()` 追加 AND 条件 |
| 收藏过滤 | `ClipboardFilters` 新增 `favorite_only`，DB 新增 `favorite` 列 |
| 自适应卡片高度 | 预览直接读 `full_text`，不在数据层截断 |
| 富文本格式渲染 | 预览读 `full_text` 中的 HTML，UI 层渲染 |
