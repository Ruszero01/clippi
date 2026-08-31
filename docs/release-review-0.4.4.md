# 0.4.4 发布前审查

审查日期：2026-08-31。基线为 `v0.4.3`，审查时远端为 `origin/main` 的 `12bcdbc`。本地原无未提交改动，已快进同步；本次审查不包含正式发版。历史标签 `v0.3.8` 本地与远端不一致，保留本地标签，没有强制覆盖。

## 变更范围

| 提交 | 内容 |
| --- | --- |
| `26d3689` | 放宽大图捕获限制 |
| `4401366` | Ctrl/Cmd+A 全选 |
| `2bafd31` | 0.4.4 更新记录 |
| `9b8bf06` | 嵌入样式表与富文本预览 |
| `368e042` | 显示器断开时窗口定位 |
| `07a9eca` | 隐藏窗口被系统调出后的守卫 |
| `12bcdbc` | 可配置单击/双击粘贴 |

## 已修复的问题

1. **P1：富文本选区扩大。** 保存 CF_HTML 时删除了片段标记，重新编码又把整个 body 当作选区。现在将有效字节偏移转为可持久保存的选区标记，编码时保留选区。Mac 粘贴来自 Windows 的历史时保留 head 样式并排除未选中的正文上下文。加入 Unicode、非法偏移、无完整文档上下文和跨平台往返测试。依据：[Microsoft CF_HTML 格式说明](https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format)。
2. **P1：Mac 多屏坐标不稳定。** `NSScreen.mainScreen` 随键盘焦点变化，不是固定坐标原点。屏幕枚举、工作区查询、插入符转换、位置保存及主/快速窗口定位统一使用 `screens[0]`。快速窗口也使用现存屏幕工作区兜底。依据：[Apple NSScreen.screens](https://developer.apple.com/documentation/AppKit/NSScreen/screens)。
3. **P1：Mac 测试和 lint 阻塞。** 新增的修饰键测试将 Command 当作 Windows 的 Win 键，造成两项失败；屏幕枚举中还有无意义类型转换导致严格 Clippy 失败。现按平台断言。
4. **P1：大图内存及 Mac TIFF 解码。** 原来允许 5.12 亿像素，DIB 路径缺少解码内存约束，Mac TIFF 又因输出与中间缓冲区共用预算而拒绝有效长截图。现在支持 1.28 亿像素、最长单边 100000，限制输出及解码工作区各 512 MiB、待处理载荷总量 512 MiB。保留 `2000×40000` 支持，并增加真实 PNG/TIFF 解码压力用例。超出限制的内容仍会被拒绝，这不是无限制的大图支持。
5. **P2：富文本样式泄漏。** `br`、`img` 等空元素被压入样式栈，而结束标签不按名称出栈，导致后续普通文字错误继承样式。现按标签匹配，跳过空元素；属性读取支持等号周围空白，避免误读 `data-class` 或引号内文字。旧回归测试转为覆盖实际生产解析器，并移除废弃解析器。CSS 仍是颜色、背景色、字重、斜体及简单标签/类选择器的有限子集，不是完整浏览器排版引擎。
6. **P2：极端比例缩略图。** 超宽图片可能算出零高度，超长图片可能超过 GPU 纹理尺寸。缩略图现在至少 1×1，且不超过 310×8192。
7. **P2：Windows 拓扑变化边界。** 单个显示器查询失败时丢弃不完整快照；没有剩余屏幕时停止快速窗口定位；隐藏窗口时取消未完成的迁移任务；主窗口迁移不改变前后层级；快速窗口按目标屏幕 DPI 计算夹取尺寸。原本针对 Windows/GPUI 的可见性守卫不机械移植到 AppKit。
8. **发布配置。** 包及 bundle 版本统一为 0.4.4，补充更新记录。移除删除 release 并清理标签的流程，重试时更新现有 release，创建时要求标签已经存在。新增三平台格式、严格 lint、单元测试工作流，发布计划依赖验证成功，并串行化同一 ref 的发布任务。Runner 标签参考 [GitHub 官方列表](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)。

## 范围判断

- Cmd+A 与单击粘贴的生产分派已使用 GPUI 平台修饰键；Mac 的主要回归在测试假设，未改动默认双击行为。
- `paste_click_mode` 使用 serde 默认值，老配置可升级；非法值回退双击。它仍是设备本地偏好，未擅自扩大配置同步白名单。
- 无数据库 schema 变更，无新增运行时依赖。修复不能恢复此前已丢失的 HTML 选区信息。
- Mac 显示器热插拔没有 Windows 的 HWND/GPUI 行为，不增加 Windows 专用守卫；实际多显示器热插拔仍需实机验证。

## 验证结果

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | 通过 |
| `cargo test --locked --quiet` | 654 通过、0 失败、2 跳过；一项为原有机器计时基线，新增压力测试单独执行 |
| 80 MP PNG/TIFF 压力测试 | 单独执行通过，耗时 55.45 秒；发版标签的 Apple Silicon CI 必跑 |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | 通过 |
| Actionlint 1.7.12 | 两份 workflow 均通过 |
| `dist plan --tag v0.4.4` | 正确识别 0.4.4 与三个目标 |
| 正式脚本提取 0.4.4 发布说明 | 通过，不混入 0.4.3 |
| Windows 屏幕枚举模块目标编译 | 通过；仅验证该模块及其测试的 Windows API 类型 |
| `cargo build --release --locked --target aarch64-apple-darwin` | 最终代码通过，设置 `MACOSX_DEPLOYMENT_TARGET=12.0` |
| `cargo build --release --locked --target x86_64-apple-darwin` | 通过，设置 `MACOSX_DEPLOYMENT_TARGET=12.0` |
| 两种 Mac bundle 的 Info.plist、ad-hoc 签名、DMG 完整性 | 按正式 workflow 的打包脚本生成并通过校验 |
| 最低系统版本 / 动态库 | 两种架构均为 macOS 12.0；24 个动态依赖均为系统库，无 Homebrew 路径 |
| 结束时远端复核 | 仍为 `12bcdbc`，未漏审新增远端提交 |

## 本地安装包

产物位于 `target/release-review-0.4.4/`，未替换当前安装，未上传。

| 架构 | DMG 路径 | SHA-256 |
| --- | --- | --- |
| Apple Silicon | `aarch64/Clippi_aarch64.dmg` | `194d8a0e95bd3f71891507fbac3a940fc7c63907583fc30d011a5c6669829142` |
| Intel | `x86_64/Clippi_x86_64.dmg` | `f622684948b41c273eb8d116e3efd889e91794ca5e84a987abc9347e30889736` |

## 发布前仍须完成

- 在原生 Windows CI 跑完整构建、测试和 lint。本机虽安装 Rust Windows target，但缺少 Windows SDK/C 头文件；完整交叉检查在依赖 `ring` 的 `assert.h` 处失败，不能当成 Windows 已验证通过。新的屏幕枚举模块已单独通过 Windows target 的编译检查，但不覆盖整个应用。
- Windows 可见性守卫在 200ms 轮询中补救隐藏状态，单元测试不能证明完全无闪现或不抢焦点；更新记录不再承诺零闪现。
- Windows 与 Mac 实机回归：隐藏/可见/置顶状态下热插拔不同缩放比例显示器；快速窗口定位及焦点；单击/双击、Cmd/Ctrl/Shift 组合；搜索/编辑框中的全选；Word/WPS 部分选区与多单元格粘贴。
- Mac 安装包继续沿用 ad-hoc 签名，未增加 Developer ID 签名或公证；这是既有分发限制。
- 当前正在运行的 Clippi 未被关闭或替换，因此不会把旧进程的运行表现当作新构建的 GUI 验证。
