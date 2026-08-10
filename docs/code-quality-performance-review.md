# Clippi 第二批性能优化单次执行方案（使用路径收敛版）

日期：2026-08-10
问题识别基线：`e9c4668636e14c43fb418931a8aba2e2d6589b05`（优化前原始代码）
执行基线：`e01d318640c18ddbef250117f2418408bc52994e`（上一批优化完成提交）
执行分支：`perf/p1-optimizations`
文档状态：已完成

## 1. 上一批优化摘要

提交 `e01d318` 已完成四项优化：卡片渲染复用文件状态缓存、窗口几何保存防抖与原子写入、剪贴板入库查询止损、搜索输入防抖与拼音匹配分配收敛。最终生产代码净增 169 行，修改 8 个生产模块，无新增依赖、轮询循环、常驻线程或缓存调度状态机；`cargo test` 为 508 passed、0 failed，Clippy 零告警，`git diff --check` 通过。

上一批的真机手工验证仍由用户执行，包括窗口拖动、断开映射盘路径、快速搜索和连续复制场景。本文不再保留旧方案的逐步实施记录；以上摘要是旧文档的唯一保留内容。

## 2. 选题原则与结论

本批问题必须在原始代码基线 `e9c4668` 中已经存在。不能把 `e01d318` 为完成上一批优化而新增的聚合查询、状态字段、测试辅助逻辑或调用结构当作新的优化理由。

原始代码中存在一条独立且高频的放大链：

1. `AppState::touch_item_usage()` 先按 ID 读取完整条目，再更新 `updated_at`，随后调用 `reload_items()`。
2. 原始 `reload_items()` 会重新加载当前页、批量填充标签，并额外执行四次标题栏状态/计数查询。
3. 单条复制、粘贴、粘贴为纯文本、图片位图、OCR、颜色转换、图片路径和文件路径等操作都经过该路径。
4. `batch_paste()` 对每个条目逐次调用 `touch_item_usage()`，因此 N 个条目会触发 N 次完整列表刷新。
5. `ClipboardListView::sync_items_from_state_for_usage()` 随后又克隆全部可见条目并重算全部卡片高度。

`e01d318` 将四次标题栏查询合并为一次，也减少了搜索和入库的其他开销，但上述“使用一次就完整刷新、批量使用重复刷新”的根因仍然存在。因此本批只优化复制/粘贴后的使用时间更新，不扩展到通用列表增量框架。

## 3. 本批优先级

### P0：建立 10,000 条历史数据的使用路径基线

先建立可重复的测试基线，再改生产代码。基线只覆盖本批路径：

- 10,000 条普通文本历史中的单条 `touch_item_usage`。
- 20 条和 100 条已选条目的批量使用时间更新。
- 默认按 `updated_at` 排序、按 `created_at` 排序、关键词搜索且收藏优先三种顺序。
- 记录预热后至少 10 次运行的中位耗时。完整刷新次数用可观察信号检测：`reload_items()` 会刷新 `clearable_history_count` 等标题栏统计，因此先删除列表外一条且不 reload，再执行使用更新，若计数变化即证明发生了完整刷新。数据库读写次数不逐条插桩（rusqlite 仅启用 `bundled`，无 trace/hooks feature，本批不新增依赖或 feature），按代码结构记录为每次使用更新的 SQL 语句构成。
- 批量场景：优化前以“循环单条 touch”等价序列测量（优化前 `batch_paste` 的 touch 循环正是该序列）；P1 将 `touch_item_usage` 从按 ID 改为按条目后，基准单条调用点做一行适配；P2 完成后批量场景改为调用共享批量方法（即优化后 `batch_paste` 的真实路径），前后统计口径各自对应本阶段真实路径。`batch_paste` 本身含真实剪贴板写入、Ctrl+V 模拟和固定 sleep，不进入自动测试。

使用 `#[ignore]` 的测试内基准或等价的测试辅助代码，不新增 Criterion 等依赖，不把机器耗时写成 CI 硬阈值。实施结果只报告本机基线和结构性调用变化，不承诺跨机器的固定百分比。

### P1：单条使用时间更新改为内存增量更新

消除每次复制/粘贴后的完整 `reload_items()`：

1. 已经为复制/粘贴读取出的 `ClipboardItem` 直接提供同步范围判断，不再由 `touch_item_usage()` 对同一 ID 重复 `get_by_id()`。注：`paste_ocr` 需把已读取条目保留出 match 作用域供 touch 复用；该路径因 OCR 文本写入 `rich_data` 仍保留现有 `reload_items()`，本批只移除 touch 引入的额外完整刷新。
2. 数据库成功更新 `updated_at` 后，只修改 `AppState.items` 中相同 ID 的时间。
3. 按 `updated_at` 排序时，在当前内存结果内稳定重排；按 `created_at` 排序时不移动条目。
4. 关键词搜索且启用收藏优先时，保持“收藏组在前、组内按更新时间降序”的现有语义；分组条件与 `load_keyword_filtered_items` 的 prioritize 判定一致（`search_favorites_first && filters.has_keyword() && !filters.is_favorites_active()`），收藏/非收藏仅在各自组内稳定重排。
5. 条目不在当前内存结果中时只更新数据库，不把它插入当前筛选结果，也不触发完整 reload。
6. 数据库更新失败时，不更新内存时间、不重排、不设置 `sync_dirty`。

本步骤只处理“使用时间”这一种不改变筛选成员关系的字段。收藏、标签、备注、编辑、删除、同步合并和新剪贴板入库继续走各自现有刷新路径。

### P2：批量使用合并为一次数据库事务和一次内存重排

`batch_paste()` 不再对每个条目重复执行单条刷新：

1. 复用批量粘贴开始时已经读取出的完整条目，统一判断是否需要标记同步。
2. 为本批条目生成同一个 `updated_at`，在一个事务内按最多 500 个 ID 一组执行有界 `UPDATE ... WHERE id IN (...)`。
3. 数据库事务成功后，一次性更新内存中的命中条目，并只稳定重排一次。
4. 事务失败时整批内存状态保持不变；剪贴板写入和粘贴流程沿用现有错误处理，不顺带重构。
5. 空 ID、重复 ID 和当前筛选结果之外的 ID 必须安全处理；重复 ID 不能产生重复内存条目。

实现注记：`Database` 新增 `touch_items(&[i64], now)`，在 `unchecked_transaction` 内按最多 500 个 ID 一组执行有界 `UPDATE ... WHERE id IN (...)`（模式同现有 `delete_items_in_chunks`）；`touch_item` 保留，仅供标签变更路径（`add_item_tag`/`remove_item_tag`/`clear_item_tags`）使用。`updated_at` 由 `AppState` 生成一次，同时用于 DB 写入与内存更新，保证两侧一致。空 ID 列表直接成功返回；重复 ID 在调用前按序去重。单条与批量共用 `AppState` 的批量 touch 方法（单条传入单元素切片），批量语义可在测试中直接覆盖，无需调用含剪贴板副作用的 `batch_paste`。

### P3：使用操作后的列表视图只同步变化项和顺序

在 P1、P2 稳定后，消除 `sync_items_from_state_for_usage()` 的全量条目克隆和全量高度重算。`action_paste`（Ctrl+Enter/搜索栏批量粘贴主路径）目前调用全量 `sync_items_from_state`，同样切换为增量同步，否则批量粘贴主路径仍存在全量克隆与全量高度重算：

1. 单条操作只从 `AppState` 克隆受影响条目；批量操作只克隆受影响集合。

实现注记：受影响 ID 集合由 `AppState` 在使用更新成功后累计记录（按序去重，直到列表消费），`sync_items_from_state_for_usage` 通过 `take` 一次性消费，UI 侧全部调用点无需传参。增量同步只把 `AppState` 中受影响条目的 `updated_at` 复制到本地同 ID 条目并按相同规则重排，`item_sizes` 保持不变（正文、备注、标签与内容尺寸均未变化，卡片高度不变）；OCR 缓存等同时改变条目内容的操作显式要求一次全量同步。
2. 正文、备注、标签和内容尺寸均未变化，因此沿用原卡片高度，只同步 `updated_at` 与顺序。
3. 条目和 `item_sizes` 必须成对移动，选择 ID、锚点和滚动位置继续按现有逻辑恢复。
4. 如果列表与 `AppState` 的 ID 集合不一致、处于传输站虚拟列表或无法证明增量同步安全，允许本次操作回退一次现有全量同步。
5. 回退只发生一次，不能重新引入按条目循环的全量同步。

停止条件：如果 P3 需要把 `AppState.items` 和 `ClipboardListView.items` 全部改成 `Arc`/`Rc`、修改 `ClipboardItem` 的公共所有权模型或建立通用列表 diff 引擎，则跳过 P3；P1、P2 仍可独立交付。

## 4. 硬性范围约束

- 生产代码相对 `e01d318` 净增不超过 **350 行**。
- 修改的生产模块不超过 **3 个**：`src/core/db.rs`、`src/state/app.rs`、`src/ui/clipboard_list.rs`。
- 不新增第三方依赖、数据库表、迁移版本或索引。
- 不新增后台线程、GPUI 轮询循环、任务队列、generation 或缓存状态机。
- 不改变剪贴板写入、校验、延迟粘贴、焦点恢复和 `skip_next` / `batch_pasting` 协议。
- 不改变同步范围、墓碑或 `updated_at` 作为使用排序依据的现有语义。
- 测试只覆盖本批使用路径，不为通用增量架构建立测试矩阵。

任一优化项超过预算或触发停止条件时，撤销该项或保留更小的止损版本，不扩大模块范围。

## 5. 明确不做

以下项目即使合理，也不进入本批：

- 不把所有 `reload_items()` 调用改造成通用脏标记或增量刷新系统。
- 不处理新剪贴板入库后的列表增量合并。
- 不拆分 `AppState`、`ClipboardListView` 或数据库大模块。
- 不实现后台搜索、只读数据库连接池、SQLite FTS5 或 keyset pagination。
- 不继续调整上一批新增的标题栏聚合查询；本批只通过移除使用路径上的完整 reload 避免调用它。
- 不统一缩略图、应用图标、favicon 或缓存清理生命周期。
- 不处理孤儿缓存清理竞态。
- 不修改 release profile，也不做安装包体积优化。
- 不为降低条目克隆而全面改造 `ClipboardItem` 所有权。

## 6. 实施顺序与停止规则

1. 核对 HEAD 为 `e01d318`，确认除本文档外工作区无改动。
2. 先加入 P0 的忽略型基准并记录优化前结果。
3. 完成 P1，运行单条使用、排序、筛选和同步范围定向测试。
4. 完成 P2，运行批量事务、失败原子性、去重和排序测试。
5. 仅在代码预算仍充足且无需改变所有权模型时完成 P3；否则明确记录为跳过。
6. 重跑同一 P0 基准，记录优化后结果和结构性调用变化。
7. 执行全量测试和静态检查。

基准若无法稳定复现完整刷新放大，停止生产代码修改并记录结果；不得为了证明方案有效而继续扩大样本、引入 profiler 框架或改造其他路径。

## 7. 验收条件

### 7.1 结构性验收

- 单条使用：复用操作前已读取的完整条目，执行一次时间更新；不额外读取同一条目，不调用 `reload_items()`、`reload_tags()` 或标题栏统计。
- 批量使用：N 个条目不再产生 N 次完整刷新；数据库时间更新在一个事务内按固定上限分块，内存只重排一次。
- 单条和批量更新失败时，数据库与内存不会形成部分成功状态。
- 当前筛选成员关系不变；隐藏条目不会因为使用时间更新进入列表。
- 默认更新时间排序、创建时间排序、收藏优先搜索的顺序与原语义一致。
- P3 若实施，正常使用路径不再克隆全部 `ClipboardItem` 或重算全部卡片高度；不安全场景最多回退一次。

### 7.2 自动验证

定向测试至少覆盖：

- 单条使用更新时间并在默认排序中移动到正确位置。
- `sort_by_created = true` 时只更新时间、不改变顺序。
- 收藏优先搜索保持分组，条目只在所属组内移动。
- 当前列表中不存在的条目不会被插入。
- 批量 ID 去重、500 边界、跨分块和事务失败原子性。
- 同步范围内与范围外条目的 `sync_dirty` 行为。
- P3 的条目/高度成对移动、选择恢复和安全回退。

批量事务与失败原子性通过 `Database::touch_items` 和 `AppState` 的共享批量方法直接测试（用触发器拒绝写入模拟失败）；不调用 `batch_paste` 本身，避免自动测试污染系统剪贴板或触发真实粘贴。

最终运行：

```powershell
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo test usage_performance_baseline -- --ignored --nocapture
git diff --check
git diff --stat e01d318
```

要求测试 0 failed、Clippy 零告警、`git diff --check` 无错误，并满足代码与模块预算。Codex 不主动启动或关闭 Clippi。

### 7.3 用户手工验证

由用户使用其当前运行实例或明确启动的测试版本验证：

1. 默认按更新时间排序时，复制或粘贴旧条目后，该条目移动到预期位置。
2. 按创建时间排序时，复制或粘贴不会改变列表顺序。
3. 在类型、标签、收藏和关键词筛选下使用条目，筛选成员关系不发生跳变。
4. 开启“搜索时收藏优先”后，收藏与非收藏分组保持稳定。
5. 批量粘贴 20 个条目，确认顺序、选择、滚动、实际粘贴内容和同步状态正常。
6. 通过全局条目热键使用当前筛选之外的条目，确认当前列表不会错误插入该条目。

## 8. 交付结构

建议在一次执行范围内按以下逻辑拆分提交：

1. `test: add clipboard usage performance baseline`
2. `perf: update clipboard usage timestamps in place`
3. `perf: batch clipboard usage updates`
4. `perf: sync usage changes without cloning the full list`（仅 P3 实施时）

最终说明只记录：原始基线证据、完成或跳过的优先级项、优化前后基准、自动验证结果和待用户手工验证项。不得追加逐轮修补流水账。

## 9. 完成定义

只有同时满足以下条件，才可标记本批完成：

- 候选问题已确认存在于原始代码 `e9c4668`，不是上一批实现产生的新问题。
- P0、P1、P2 完成；P3 完成或按停止条件明确跳过。
- 单条和批量使用路径均不再因更新时间更新触发完整数据库列表刷新。
- 未引入通用列表 diff、全局所有权改造、后台数据库或新并发协议。
- 自动验证全部通过，代码和模块预算未超限。
- 文档只保留一份最终实施结果，性能结论不超出实际测量数据。

## 10. 实施结果

### 10.1 完成范围

P0、P1、P2、P3 全部完成，无跳过项，未触发停止条件：

- **P1**：`touch_item_usage(id)` 改为按条目传入（`&ClipboardItem`），复制/粘贴各调用点复用已读取条目；`paste_ocr` 将条目保留出 match 作用域（该路径因 OCR 文本写入 `rich_data` 仍保留现有 reload）。数据库更新成功后只修改 `AppState.items` 中相同 ID 的时间并按 `updated_at` 稳定重排（`created_at` 不移动；收藏优先搜索仅组内重排，分组条件与 `load_keyword_filtered_items` 的 prioritize 一致）；条目不在当前筛选结果时只更新数据库。数据库失败时不更新内存、不重排、不设置 `sync_dirty`。
- **P2**：新增 `Database::touch_items(&[i64], now)`，在 `unchecked_transaction` 内按最多 500 个 ID 一组执行有界 `UPDATE ... WHERE id IN (...)`（模式同 `delete_items_in_chunks`）；`touch_item` 保留，仅供标签变更路径使用。`AppState::touch_items_usage` 为单条与批量共用入口（单条传单元素切片），批量 = 一个事务、一个 `updated_at`、一次内存重排、按序去重；`batch_paste` 改为一次调用。
- **P3**：`sync_items_from_state_for_usage` 改为增量同步——`AppState` 累计尚未消费的使用 touch ID，UI 一次消费后只复制受影响条目的 `updated_at`，并按与 `AppState` 相同的规则**成对重排 items 与 item_sizes**（卡片高度沿用原值）；重排前后按条目 ID 恢复选择、锚点和悬停目标。`action_paste`（Ctrl+Enter/搜索栏批量粘贴主路径）同步切换为增量。只有可见 ID 集合完全一致、尺寸缓存长度一致、受影响 ID 全部可见且未发生内容级变化时才走增量；transfer 视图、OCR 新写入 `rich_data` 或其他不安全场景回退一次全量同步。普通全量同步会清除已被覆盖的待处理使用请求，避免后台热键产生的多次更新相互覆盖或重复消费。
- 未引入通用列表 diff、所有权改造（`Arc`/`Rc`）、后台数据库或新并发协议；生产模块仅 `src/core/db.rs`、`src/state/app.rs`、`src/ui/clipboard_list.rs` 三个。

### 10.2 优化前后基准（本机，10,000 条历史、列表窗口 200，预热后 10 次中位耗时）

| 场景 | 优化前 | 优化后 |
| --- | --- | --- |
| 单条使用（`updated_at` 排序） | 6259 µs（含完整 reload） | 45 µs |
| 单条使用（`created_at` 排序） | 6284 µs | 38 µs |
| 单条使用（收藏优先搜索） | 40805 µs | 46 µs |
| 批量 20 条 | 813000 µs（20 次完整刷新） | 124 µs |
| 批量 100 条 | 4083387 µs（100 次完整刷新） | 464 µs |

完整刷新检测（删除列表外一条后使用条目，观察 `clearable_history_count` 是否被 reload 刷新）：优化前每次使用更新均触发完整刷新，优化后不再触发。批量场景口径：优化前为 `batch_paste` 的逐条 touch 等价序列，优化后为共享批量方法（`batch_paste` 真实路径）。性能结论仅限本机，不承诺跨机器比例。

### 10.3 自动验证

- `cargo test`：并行全量有一次 526 passed、0 failed、1 ignored（机器计时基准）；后续串行全量命中 `config_snapshot_upload_accepts_created_response`（WebDAV 既有测试，本批未触碰该模块）的既有 socket 时序失败，结果为 525 passed、1 failed、1 ignored。该失败已在执行基线 `e01d318` 上同样复现，本次单独连续运行 5 次均通过，因此不扩展本批范围修改 WebDAV 模块。
- `cargo clippy --all-targets --all-features -- -D warnings`：零告警。
- `cargo test usage_performance_baseline -- --ignored --nocapture`：见 10.2。
- `git diff --check`：通过。
- 新增定向测试 19 个：单条移动 / `created_at` 保持 / 收藏分组内移动 / 隐藏条目不插入 / 批量去重与一次重排 / 同步范围内外 `sync_dirty` / DB 层 `touch_items` 跨分块与事务失败回滚（触发器）/ AppState 失败原子性与待同步 ID 累计消费 / UI 层条目与高度成对移动、交互索引恢复、可见 ID 集合与尺寸缓存安全门、内容变化强制回退。
- 生产代码净增 278 行（不含测试），未超 350 行预算；生产模块数 3。

### 10.4 待用户手工验证

按 7.3 六项执行。另请注意：构建链接 `clippi.exe` 时若本机 Clippi 正在运行，会因文件占用失败（`os error 5`），请先关闭运行中的实例再执行 `cargo build`。

### 10.5 提交建议

按第 8 节拆分：`test: add clipboard usage performance baseline`（P0 基准）；`perf: update clipboard usage timestamps in place`（P1）；`perf: batch clipboard usage updates`（P2）；`perf: sync usage changes without cloning the full list`（P3）。
