<div align="center">
  <p>
    <img src="assets/LOGO_notext.png" width="120" alt="Clippi 图标">
  </p>

  # Clippi

  轻量级剪贴板管理器 · 基于 Rust + GPUI 构建<br>
  支持 Windows 和 macOS

  <p>
    <a href="README.md">中文</a> · <a href="README_EN.md">English</a>
  </p>

  <p>
    <a href="https://github.com/Ruszero01/clippi/issues">反馈问题</a> ·
    <a href="https://github.com/Ruszero01/clippi/releases">更新日志</a>
  </p>

  <p>
    <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License"></a>
    <img src="https://img.shields.io/badge/Rust-2021-%23000000?logo=rust" alt="Rust">
    <img src="https://img.shields.io/badge/GPUI-0.2-%23555555?logo=rust" alt="GPUI">
    <img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-blue" alt="Platform">
  </p>
</div>

<p align="center">
  <img src="assets/UI.png" alt="UI Screenshot" width="100%">
</p>

---

## 为什么是 Clippi？

- GPUI 原生 GPU 渲染，无需 webview 进程支持，低资源占用与美观度并存
- Rust 后端响应快速，具备良好的性能和跨平台能力
- 多后端云同步架构，支持OneDrive/iCloud/WebDAV，低门槛易扩展

## Clippi 能做什么？

### 剪贴板监控
- 多格式内容检测（按优先级）：
  - **文件** — 文件/文件夹路径，支持多文件，提取系统图标；单图片文件自动识别为图片
  - **图片** — 记录图片自动处理缩略图节省内存；支持 OCR 文字识别（Windows Media Ocr / macOS Apple Vision，零额外依赖）；支持 QR 码自动识别（常规/反色二维码）
  - **链接** — URL 自动识别 (http/https)，支持域名/路径提取、favicon 预览
  - **颜色** — HEX/RGB 颜色自动检测与归一化
  - **富文本** — HTML、RTF 格式
  - **纯文本** — 普通文本内容；自动识别邮箱地址和电话号码
  - **路径** — Windows 绝对路径 / UNC 路径 / Unix 绝对路径智能识别
- 内容哈希去重：相同内容重复复制时更新时间戳，不重复记录
- 颜色归一化去重：`#FF8000` ≡ `rgb(255,128,0)`，避免重复
- 图片 OCR 缓存：同一图片不重复识别，OCR 文本参与关键词搜索
- 快捷键黑名单：指定应用中禁用全局热键
- 纯文本复制模式：开启后丢弃富文本格式，仅保留纯文本
- SQLite (WAL 模式) 本地持久化，支持自定义数据库路径

### 内容管理
- 双击卡片快速粘贴到上一个活动窗口
- 右键菜单（单条目/批量双模式）：
  - 复制、粘贴、编辑、备注
  - 颜色条目：粘贴为 RGB / 粘贴为 HEX
  - 图片条目：打开原图、粘贴 OCR 文本、识别 QR 码
  - 收藏/取消收藏、删除
  - 标签管理（添加/移除/批量操作）
- 编辑面板：全文本编辑器 + 类型选择器（6种类型）+ URL 解码 / JSON 格式化 / Base64 解码 / 文本修整工具栏按钮
- 多选批量操作（Ctrl/Shift 选择）：批量粘贴（换行分隔）、批量收藏、批量删除、批量标签
- 六级类型筛选：文本 / 富文本 / 图片 / 文件 / 链接 / 颜色
  - 链接 ⇄ 路径、文件 ⇄ 图片双向自动联动
- 关键词搜索 — 同时匹配文本内容和标签名
- 标签筛选 — 多标签 AND/OR 逻辑可切换，与其他筛选维度 AND 组合
- 排序：按创建时间 / 按最后使用时间
- 备注内联编辑 + 全内容编辑器
- 敏感信息预览脱敏：邮箱仅显示前两位 + 域名（`jo***@gmail.com`），手机号仅显示前三位 + 后四位（`138****5678`），复制/粘贴/搜索仍使用完整内容

### 标签系统
- 创建/编辑/删除标签、12 种预设颜色
- 标签关联到剪贴板条目（多对多）
- 侧边标签栏：筛选标签固定到窗口左侧，支持展开/折叠动画和固定
- 标签筛选面板 + 标签选择器面板（均支持标签 CRUD）
- 单条目/批量标签分配与移除
- 跨设备标签同步（含颜色冲突解决）

### 窗口与交互
- 全局快捷键呼出/隐藏窗口（默认 `Alt+V`，支持录制自定义）
- 窗口置顶（固定）模式
- 失焦自动隐藏（可配）
- 多显示器支持（光标所在屏幕）
- 三种窗口呼出位置：居中 / 跟随鼠标 / 记住位置
- 拖拽调整窗口大小（右边缘 + 底边缘 + 右下角）
- 窗口尺寸跨会话持久化
- 深色/亮色/跟随系统 三主题，自动检测系统深色模式
- Toast 通知 + 设置错误滚动警告
- 版本更新检查：托盘菜单显示当前版本，自动检查 GitHub 更新，一键跳转下载

### 显示选项
- 来源应用信息显示（剪贴板来源程序名称和图标）
- 卡片高度模式：高 / 中 / 低 / 自适应
- 链接 favicon 网站图标预览
- 文件/路径类型系统图标
- 悬停显示原始内容（有备注时）
- 自动滚动到顶部（窗口呼出时）
- 纯文本复制模式

### 云同步
- 多后端架构：支持同时配置多个同步服务，每个后端独立开关与独立同步间隔
- 本地文件夹后端：通过 OneDrive / iCloud 云盘同步
- WebDAV 后端：支持任意 WebDAV 服务器（如 NextCloud、ownCloud、坚果云），ETag 缓存 + Basic Auth
- 自动检测 OneDrive (Windows + macOS) 和 iCloud (macOS) 预设路径
- 跨设备删除与取消收藏传播（墓碑机制，30 天窗口）
- 最后写入者胜出 (LWW) 冲突解决
- 语义哈希比较，跳过无变更推送（避免同步循环）
- 冲突文件自动合并与清理
- 可配同步间隔（30秒 / 1分 / 10分 / 30分）+ 手动即时同步
- 仅收藏条目同步模式
- 异步连接测试，不阻塞 UI

### 设置
- 通用：开机自启、失焦隐藏、静默启动、主题模式、窗口位置、界面语言
- 剪贴板：排序、卡片高度、来源应用、自动滚顶、纯文本复制、悬停原文、OCR 识别开关、QR 码识别开关
- 快捷键：呼出快捷键录制、应用黑名单管理
- 数据：数据库路径自定义与迁移、最大保存条目数
- 同步：自动同步开关、间隔、仅收藏模式、多后端管理（增/删/改）、独立后端开关与间隔

## 技术栈

| 组件 | 技术 |
|------|------|
| UI 框架 | [GPUI](https://www.gpui.rs/) 0.2 |
| 剪贴板 | [clipboard-rs](https://github.com/ChurchTao/clipboard-rs) |
| 数据存储 | [rusqlite](https://github.com/rusqlite/rusqlite) (bundled SQLite, WAL 模式) |
| 系统托盘 | [tray-icon](https://github.com/tauri-apps/tray-icon) |
| 全局快捷键 | [global-hotkey](https://github.com/tauri-apps/global-hotkey) |
| 图片处理 | [image](https://github.com/image-rs/image) |
| HTTP | [ureq](https://github.com/algesten/ureq) (favicon 获取, 版本检查) |
| 配置 | TOML ([serde](https://serde.rs/) + [toml](https://github.com/toml-rs/toml)) |
| 同步协议 | JSON (v2, [serde_json](https://github.com/serde-rs/json)) |
| 版本比较 | [semver](https://github.com/dtolnay/semver) |
| 日志 | [log](https://github.com/rust-lang/log) + [simplelog](https://github.com/drakulix/simplelog.rs) |
| Windows | [windows-sys](https://github.com/microsoft/windows-rs) + [windows](https://github.com/microsoft/windows-rs) (OCR) |
| macOS | [objc2](https://github.com/madsmtm/objc2) + [core-graphics](https://github.com/servo/core-foundation-rs) + [apple-vision](https://github.com/servo/core-foundation-rs) (OCR) |

## 构建

```bash
cargo build
cargo run
```

## 平台支持

| 功能 | Windows | macOS |
|------|---------|-------|
| 剪贴板监控 | ✅ | ✅ |
| 粘贴模拟 | ✅ | ✅ |
| 全局快捷键 | ✅ | ✅ |
| 热键录制自定义 | ✅ | ✅ |
| 快捷键黑名单 | ✅ | ✅ |
| 系统托盘 | ✅ | ✅ |
| 开机自启 | ✅ | ✅ |
| 焦点监听（自动隐藏） | ✅ | ✅ |
| 来源应用检测 | ✅ | ✅ |
| 文件图标提取 | ✅ | ✅ |
| 网站图标获取 | ✅ | ✅ |
| 多显示器支持 | ✅ | ✅ |
| 系统深色模式检测 | ✅ | ✅ |
| OneDrive 预设检测 | ✅ | ✅ |
| iCloud 预设检测 | ❌ | ✅ |
| 图片 OCR 文字识别 | ✅ | ✅ |
| 邮箱/电话识别 | ✅ | ✅ |
| 侧边标签栏 | ✅ | ✅ |
| 版本更新检查 | ✅ | ✅ |
| 编辑面板 URL 解码 | ✅ | ✅ |
| 编辑面板 Base64 解码 | ✅ | ✅ |
| 编辑面板 JSON 格式化 | ✅ | ✅ |
| 编辑面板文本修整 | ✅ | ✅ |
| 图片 QR 码识别 | ✅ | ✅ |
| 敏感信息预览脱敏 | ✅ | ✅ |
| 界面语言 (中文/英文) | ✅ | ✅ |

---

## macOS 用户须知

Clippi 未经过 Apple 开发者签名（未加入 Apple Developer Program），首次打开或每次更新后，macOS Gatekeeper 会阻止应用运行。请按以下步骤操作：

### 首次安装 / 更新后打开

1. 下载 `.dmg` 文件后，将 Clippi 拖入 `应用程序` 文件夹
2. **不要直接双击打开**，请右键（或 Ctrl+点击）Clippi 图标 → 选择 **"打开"**
3. 在弹出的对话框中点击 **"打开"**（此操作只需每次更新后执行一次）

> 或者前往 **系统设置 → 隐私与安全性**，在页面底部找到「已阻止使用 Clippi」的提示，点击 **"仍要打开"**。

### 授予辅助功能权限（快速粘贴功能必需）

Clippi 的快速粘贴功能需要辅助功能权限来模拟按键输入：

1. 打开 **系统设置 → 隐私与安全性 → 辅助功能**
2. 在列表中找到 **Clippi** 并开启开关
3. 如果 Clippi 不在列表中，点击 `+` 按钮手动添加，路径为 `/Applications/Clippi.app`

> 未授予辅助功能权限时，快速粘贴（双击卡片/Enter 键粘贴）将无法工作，但你仍然可以使用右键菜单手动复制粘贴。
