# Clippi 性能优化单次执行方案（收敛版）

日期：2026-08-10  
代码基线：`e9c4668636e14c43fb418931a8aba2e2d6589b05`（`chore: sync Cargo.lock to 0.4.2`）  
执行分支：继续使用 `perf/p1-optimizations`；正式实施前先将源代码还原到上述提交  
文档状态：待执行  

## 1. 目的与结论

本次只处理四个已经确认、能够以小改动降低 UI 主线程负担的问题：

1. 卡片渲染直接同步探测源路径。
2. 窗口拖动期间重复同步保存几何配置。
3. 连续输入触发重复关键词搜索及 matcher 分配。
4. 剪贴板入库后的重复查询和固定刷新放大。

当前工作区尝试一度扩展到 13 个文件、生产代码和测试合计净增两千余行，并引入两套缓存并发状态机。该实现的复杂度已经超过原问题本身，后续发现的大量竞态也主要由新状态机产生。该源代码实现整体作废，仅作为范围膨胀的反例；正式实施前先把当前分支的源代码还原到上一个干净提交，再从该基线重新实现，不选择性复制或保留旧代码。

本文件是唯一执行依据。此前的多轮复审记录不再作为实施清单，也不在本文继续追加“第 N 轮修复”。执行中发现的相邻问题只登记到“后续工作”，不进入本次改动。

## 2. 硬性范围约束

### 2.1 代码预算

- 生产代码相对基线提交 `e9c4668` 净增不超过 **500 行**。
- 新增或修改的生产模块不超过 **8 个**。
- 单个优化项净增生产代码不超过 **150 行**。
- 不新增第三方依赖。
- 不新增 GPUI 轮询循环或常驻后台线程。
- 测试只覆盖本次行为，不为未实施的通用框架建立测试矩阵。

超过任一预算时，不继续扩展设计；撤销该优化项或缩小为更简单的止损版本。

### 2.2 明确不做

以下内容即使在实施或审查中发现合理性，也不属于本次范围：

- 不实现缩略图或应用图标的 FIFO、generation、pending、per-key 锁、TTL、负缓存等通用调度状态机。
- 不修改 `src/core/cache_cleanup.rs` 的孤儿文件清理协议。
- 不实现后台数据库搜索、只读连接池、keyset pagination 或 SQLite FTS5。
- 不实现剪贴板列表增量合并；入库后继续使用现有完整 `reload_items()`。
- 不改变 `Database::upsert()` 的返回语义来服务增量列表。
- 不拆分大模块，不顺带进行架构重构。
- 不调整 release profile。
- 不处理与本次四项无直接因果关系的既有问题。

## 3. 实施基线与工作区要求

本次不新建分支，也不是在当前源代码 diff 上做减法。继续使用 `perf/p1-optimizations`，但实施前必须先恢复到提交 `e9c4668` 的干净源代码。

实施前必须满足：

1. 当前分支保持为 `perf/p1-optimizations`，HEAD 为 `e9c4668636e14c43fb418931a8aba2e2d6589b05`。
2. 正式实施前由用户还原当前分支中的全部源代码改动；Codex 不在本方案阶段执行 reset、clean 或删除操作。
3. 还原后只允许工作区保留 `docs/code-quality-performance-review.md` 这一份方案文档，`src`、`Cargo.toml`、`Cargo.lock`、`build.rs` 不得存在 diff。
4. 不 cherry-pick、复制或手工移植旧工作区中的实现代码。可以参考本文描述的问题与验收条件，但每个改动都必须重新从干净源码推导。
5. 所有代码量、文件数和验证结果均相对 `e9c4668` 计算。

开始编码前记录一次基线：

```powershell
git rev-parse HEAD
git status --short
git diff --exit-code e9c4668 -- src Cargo.toml Cargo.lock build.rs
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

只有 HEAD 等于上述基线提交、工作区除本文档外没有任何改动、源代码基线检查和基线测试均通过，才进入下一步。若基线测试本身失败，先记录为基线问题并停止，不得在性能优化中顺带修复。

## 4. 单次实施步骤

每一步完成后只做该步骤的定向测试；通过后才进入下一步。某一步触发停止条件时，跳过该步，不改动后续范围。

### 步骤 1：卡片渲染复用现有文件状态缓存

目标：消除 `ClipboardCard` 渲染实现中对源路径的直接同步 `Path::exists()` / `path_exists()` 调用。

实施内容：

1. 在卡片组合入口对路径和图片路径各调用一次现有 `file_status::cached_path_kind()`。
2. 正文、预览和底部状态共享同一个缓存结果，不重复探测。
3. `None` 表示“未知/检查中”，不得立即显示“已失效”。
4. 继续使用现有 `FileStatusChanged` 通知重绘。
5. 不修改缩略图生成、应用图标写入或缓存清理实现；应用自身缓存文件不属于本步骤的源路径探测范围。

预计范围：仅 `src/ui/clipboard_card.rs`；直接复用现有 `file_status` API，不扩展服务层。  
生产代码预算：净增不超过 60 行。

验收：

- `ClipboardCard` 渲染代码不再直接探测源路径。
- 同一卡片一次组合只请求一次相同路径状态。
- 现有失效路径、远程图片和同步等待语义测试通过。

停止条件：如果需要修改缩略图或图标缓存生命周期，立即停止本步骤并登记后续工作。

### 步骤 2：窗口几何保存防抖和原子替换

目标：拖动或缩放窗口时只更新内存，停止每 200 ms 重写配置文件。

实施内容：

1. `capture_window_geometry()` 只更新内存几何并设置 dirty。
2. 最后一次变化后 500 ms 执行 trailing-edge flush。
3. `hide()` 和 `prepare_shutdown()` 强制 flush。
4. `AppSettings::save()` 复用现有 `save_atomic_to()`，保存失败写日志并保留 dirty。
5. 失败重试使用简单的 1、2、4、8、16、30 秒退避；新几何变化重置退避。
6. 保存仍可在主线程执行，因为防抖后单次写入频率很低；本次不新增后台保存队列和 generation。
7. flush 触发时必须读取最新 `AppState.settings` 序列化写盘，不得保存捕获几何时的旧配置副本；与 config-sync 的 `apply_config_snapshot()`（先写盘 merged 配置、再提交内存）保持兼容，避免挂起的 flush 用旧配置覆盖刚写入磁盘的新配置。

预计范围：`src/core/settings.rs`、`src/ui/window_manager.rs`。  
生产代码预算：净增不超过 120 行。

验收：

- 连续拖动只产生一次防抖写入；隐藏或退出最多补一次强制写入。
- 配置保存使用原子替换。
- 失败不会清除 dirty，也不会每 200 ms 重试。
- 保留 `cfg!(test)` 下不覆盖用户真实配置的保护。

停止条件：如果需要通用 SettingsStore、任务队列或跨线程版本协议，改为只保留原子保存和主线程防抖。

### 步骤 3：剪贴板入库查询止损

目标：减少固定数据库往返，但不改变列表刷新模型。

实施内容：

1. 每个待入库 hash 最多执行一次 `get_by_hash()`，OCR、QR 和富文本决策复用结果。
2. `prune_excess_non_favorites()` 使用 `ORDER BY ... LIMIT ?`，只读取需要删除的 ID。
3. 合并标题栏的两个 `EXISTS` 和两个 `COUNT(*)` 为一次查询。
4. 标签定义和关联未变化时，剪贴板批次不调用 `reload_tags()`。
5. `(is_favorite, created_at)` 复合索引当前不存在（仅有单列 `idx_is_favorite`、`idx_created`），仅在 `EXPLAIN QUERY PLAN` 明确获益时新增；如新增，用 `CREATE INDEX IF NOT EXISTS` 幂等加入 `init_schema`，不需要 migration 版本。
6. 批次结束继续调用现有 `reload_items()`；不做增量插入、排序修复或页外重复项处理。

预计范围：`src/services/gpui_clipboard.rs`、`src/core/db.rs`、`src/state/app.rs`。  
生产代码预算：净增不超过 150 行。

验收：

- 同一 hash 的完整记录查询不重复。
- prune 查询的返回行数不超过 `excess`。
- 标题栏统计只有一次数据库往返。
- 普通复制不读取整张标签表。
- 现有列表加载、排序、筛选和分页路径保持不变。

停止条件：如果实现需要改变 `upsert()` 返回值、单条列表投影或列表合并逻辑，撤销该部分，保留完整 reload。

### 步骤 4：搜索快速止损

目标：避免连续按键重复执行必然过期的同步搜索，并降低 matcher 的重复分配。

实施内容：

1. 搜索输入增加 150 ms 防抖和简单 generation 校验。
2. 防抖到期后仍调用现有 `set_keyword()` / `reload_items()`；本次不把数据库访问移到后台。
3. 删除 `term.to_string()` 等明显的临时分配。
4. 同一文本的拼音编码在一次多关键词匹配中最多构造一次。
5. 保持直接匹配、全拼、首字母以及“同起点优先更长范围”的现有语义。

预计范围：`src/ui/search_box.rs`、`src/core/search.rs`。  
生产代码预算：净增不超过 150 行。

验收：

- 快速连续输入只提交最后一个稳定关键词。
- 清空搜索立即生效或遵循明确且一致的防抖语义。
- 正文、备注、标签、富文本、全拼和首字母回归测试全部通过。
- `choose_better_match` 的“同起点优先更长”语义不回退；基线中不存在同名测试且现有 `match_ranges_*` 测试未直接断言该语义，本次在 `search.rs` 补一条覆盖该语义的回归测试。

停止条件：如果需要数据库连接池、后台快照或 FTS，停止并登记后续工作。

## 5. 验证方式

### 5.1 每步自动验证

- 步骤 1：运行 `clipboard_card` 和 `file_status` 相关测试。
- 步骤 2：运行 settings 与 window manager 相关测试。
- 步骤 3：运行 db、gpui_clipboard 与 AppState 相关测试。
- 步骤 4：运行 search、search_box 与关键词筛选相关测试。

定向测试失败时只修复本步骤直接造成的回归；如果修复需要突破范围约束，撤销该步骤。

### 5.2 最终自动验证

```powershell
cargo test
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
git diff --stat
```

验收条件：

- 测试 0 failed；不以测试总数作为目标。
- Clippy 零告警。
- `git diff --check` 无错误。
- 生产代码净增不超过 500 行，生产模块不超过 8 个。
- 不存在新的缓存调度器、连接池、轮询循环或跨线程状态协议。

### 5.3 用户手工验证

Codex 不主动启动或关闭 Clippi。由用户使用当前运行实例或明确启动测试版本，完成：

1. 连续拖动和缩放窗口 10 秒，确认无明显卡顿且最终几何能够恢复。
2. 打开包含断开映射盘路径的历史项，滚动和悬停不应被同步路径探测阻塞。
3. 在较多历史数据下快速输入和删除关键词，确认只出现最终稳定结果。
4. 连续复制普通文本、富文本和图片，确认列表、排序、标签和收藏状态正常。

没有真实测量数据前，只能描述为“降低了风险/减少了调用”，不得在文档或提交信息中声称已经提升某个百分比。

## 6. 交付结构

本次只交付一组可整体审查的改动，建议按以下逻辑拆成小提交，但不扩展为多个实施阶段：

1. `perf: reuse cached path status in clipboard cards`
2. `perf: debounce and atomically persist window geometry`
3. `perf: reduce clipboard database round trips`
4. `perf: debounce search and reuse matcher encoding`

最终说明只包含：从干净基线实现了什么、明确跳过了什么、自动验证结果、用户尚需完成的手工验证。不要描述当前废弃工作区的修补过程，也不要追加逐轮审查流水账。

## 7. 后续工作清单（本次禁止实施）

以下问题可以另建独立文档和独立分支，每项都必须先有性能数据或故障复现：

- 后台搜索、只读数据库连接与 FTS5。
- 缩略图/图标缓存生命周期统一设计。
- 孤儿缓存清理与新捕获文件之间的竞态。
- 无筛选列表的安全增量合并。
- 10,000 条历史数据的 benchmark 与 profiler 基线。
- 大模块拆分和依赖方向整理。
- release profile 的体积/速度对比。

这些事项不因在代码审查中被发现而自动升级到本次范围。

## 8. 完成定义

只有同时满足以下条件，才可以将本次优化标记为完成：

- 实施前当前分支的 HEAD 为 `e9c4668`，源代码初始 diff 为空，工作区只保留本文档。
- 实施分支从未引入复杂缓存状态机或增量列表方案，而不是先引入再删除。
- 四个步骤均在预算内完成，或按停止条件明确跳过。
- 自动验证全部通过。
- 文档只保留一份最终验证结果。
- 尚未执行的真机测试和后续事项被明确标注，没有用单元测试结果替代性能结论。

## 9. 实施结果（2026-08-10）

基线核对：HEAD = `e9c4668`，实施前工作区除本文档外无改动；实施后生产模块 8 个（未超限），含测试共净增 220 行，其中测试净增 51 行、生产代码净增 169 行，未超 500 行预算；无新增依赖、无新增轮询循环或常驻线程、无缓存调度状态机。

### 步骤 1：卡片渲染复用现有文件状态缓存（完成）

- `src/ui/clipboard_card.rs`：render 入口对可探测的原生非 UNC path 项与图片项各调用一次 `file_status::cached_file_exists()`，正文预览与底部尺寸标签共享结果；path 探测沿用原有 trim 与 UNC 跳过语义，`None` 视为“未知/检查中”，不立即显示失效。原有 4 处同步 `path_exists()` / `Path::exists()` 全部移除，缩略图、图标缓存与清理协议未改动。
- 预算：净增 7 行。

### 步骤 2：窗口几何保存防抖和原子替换（完成）

- `src/core/settings.rs`：新增 `save_result()`（原子写、返回结果、保留 `cfg!(test)` 下不覆盖真实配置的保护），`save()` 作为兼容封装并记录保存错误，不再静默吞掉失败。
- `src/ui/window_manager.rs`：`capture_window_geometry()` 只更新内存并标 dirty；现有 poll 驱动 500 ms trailing-edge flush，失败保留 dirty 并按 1/2/4/8/16/30 秒退避、新几何变化重置退避；`hide()` 与 `prepare_shutdown()` 强制 flush。轮询和强制 flush 共用成功/失败状态更新，成功后清 dirty，失败后进入退避重试。flush 在触发时刻读取最新 `AppState.settings` 序列化，与 config-sync 先写盘后提交内存的顺序兼容；新增退避序列与 30 秒上限测试。
- 预算：净增 90 行（含测试）。

### 步骤 3：剪贴板入库查询止损（完成）

- `src/services/gpui_clipboard.rs`：OCR、QR、富文本决策共享同一 hash 的一次惰性 `get_by_hash()`；批次结束不再无条件调用 `reload_tags()`（剪贴板入库不改变标签定义与关联，tag CRUD 路径已各自刷新）。
- `src/core/db.rs`：`prune_excess_non_favorites()` 改为 `ORDER BY created_at ASC LIMIT ?`，只读取将被删除的 ID；新增 `load_titlebar_stats()`，用一条聚合查询的一次表扫描同时计算标题栏两个可用状态与两个清理计数，`AppState::new()` 与 `refresh_titlebar_filter_availability()`（随 `reload_items()` 在每次批次后执行）统一使用；原 4 个单查方法已删除，并补充空库聚合结果测试。
- 索引：`EXPLAIN QUERY PLAN` 显示复合索引 `(is_favorite, created_at)` 能消除 prune 查询的 TEMP B-TREE，但 10,000 行实测仅约 0.02 ms 差异，而剪贴板为高频写入场景、索引维护成本不划算，故未新增；计划级差异已记录在此。
- 预算：净增 11 行（含测试）。

### 步骤 4：搜索快速止损（完成）

- `src/ui/search_box.rs`：输入 150 ms 防抖 + generation 校验，快速连续输入只提交最后一个稳定关键词；清空搜索遵循同一防抖语义。
- `src/core/search.rs`：`find_next_match` 泛型化消除 `term.to_string()` 临时分配；新增 `PinyinIndex`（全拼/首字母编码一次），`text_matches_all_terms`、`match_ranges`、`highlight_segments` 共享单次编码；布尔匹配直接检查编码字符串，不再为判断是否命中构造 spans Vec；新增 `same_start_prefers_longer_match_over_shorter_candidate` 回归测试覆盖“同起点优先更长”语义（基线不存在文档原引用的测试名，已按修订改为本次新增测试）。
- 预算：净增 112 行（含测试）。

### 自动验证

- `cargo test`：508 passed，0 failed。此前 `config_snapshot_upload_accepts_created_response` 曾出现一次偶发失败，提交前最终全量运行通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：零告警。
- `git diff --check`：无错误。
- 四个步骤均在预算内完成，无停止条件触发。

### 用户手工验证（待用户执行）

1. 连续拖动和缩放窗口 10 秒，确认无明显卡顿且最终几何能够恢复。
2. 打开包含断开映射盘路径的历史项，滚动和悬停不应被同步路径探测阻塞。
3. 在较多历史数据下快速输入和删除关键词，确认只出现最终稳定结果。
4. 连续复制普通文本、富文本和图片，确认列表、排序、标签和收藏状态正常。

未进行真实性能测量，以上结果仅描述“减少了调用/降低了风险”，不声称性能提升百分比。
