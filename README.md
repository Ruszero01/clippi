<div align="center">
  <p>
    <img src="assets/LOGO_notext.png" width="120" alt="Clippi 图标">
  </p>

  # Clippi

  轻量级剪贴板管理器 · 基于 Rust + Slint 构建<br>
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
    <img src="https://img.shields.io/badge/Slint-1.16-%232374FF?logo=slint" alt="Slint">
    <img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-blue" alt="Platform">
  </p>
</div>

<p align="center">
  <img src="assets/UI.png" alt="UI Screenshot" width="100%">
</p>

---

## 为什么是 Clippi？

- 简单记录日常剪贴板内容
- 不希望占用太多系统资源
- 多设备跨平台剪贴板同步
- 拥有美观现代的用户界面

## Clippi 能做什么？

- Slint 原生前端渲染，无需 webview 进程支持，低资源占用与美观度并存
- Rust 后端响应快速，具备良好的性能和跨平台能力
- 多后端云同步架构，支持OneDrive/iCloud，低门槛易扩展（后续计划扩展WebDAV等渠道）

### 剪贴板监控
- 多格式内容检测（按优先级）：
  - **文件** — 文件/文件夹路径，支持多文件，提取系统图标；单图片文件自动识别为图片
  - **图片** — 记录图片自动处理缩略图节省内存
  - **链接** — URL 自动识别 (http/https)，支持域名/路径提取、favicon 预览
  - **颜色** — HEX/RGB 颜色自动检测与归一化
  - **富文本** — HTML、RTF 格式
  - **纯文本** — 普通文本内容
  - **路径** — Windows 绝对路径 / UNC 路径 / Unix 绝对路径智能识别
- 内容哈希去重：相同内容重复复制时更新时间戳，不重复记录
- 颜色归一化去重：`#FF8000` ≡ `rgb(255,128,0)`，避免重复
- 快捷键黑名单：指定应用中禁用全局热键
- 纯文本复制模式：开启后丢弃富文本格式，仅保留纯文本
- SQLite (WAL 模式) 本地持久化，支持自定义数据库路径

### 内容管理
- 双击卡片快速粘贴到上一个活动窗口
- 右键菜单（单条目/批量双模式）：
  - 复制、粘贴、编辑、备注
  - 颜色条目：粘贴为 RGB / 粘贴为 HEX
  - 图片条目：打开原图
  - 收藏/取消收藏、删除
  - 标签管理（添加/移除/批量操作）
- 多选批量操作（Ctrl/Shift 选择）：批量粘贴（换行分隔）、批量收藏、批量删除、批量标签
- 六级类型筛选：文本 / 富文本 / 图片 / 文件 / 链接 / 颜色
  - 链接 ⇄ 路径、文件 ⇄ 图片双向自动联动
- 关键词搜索 — 同时匹配文本内容和标签名
- 标签筛选 — 多标签 AND/OR 逻辑可切换，与其他筛选维度 AND 组合
- 排序：按创建时间 / 按最后使用时间
- 备注内联编辑 + 全内容编辑器

### 标签系统
- 创建/编辑/删除标签、12 种预设颜色
- 标签关联到剪贴板条目（多对多）
- 标签筛选面板 + 标签选择器面板
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

### 显示选项
- 来源应用信息显示（剪贴板来源程序名称和图标）
- 卡片高度模式：高 / 中 / 低 / 自适应
- 链接 favicon 网站图标预览
- 文件/路径类型系统图标
- 悬停显示原始内容（有备注时）
- 自动滚动到顶部（窗口呼出时）
- 纯文本复制模式

### 云同步
- 多后端架构：支持同时配置多个同步服务
- 本地文件夹后端：通过 OneDrive / iCloud 云盘同步
- 自动检测 OneDrive (Windows + macOS) 和 iCloud (macOS) 预设路径
- 跨设备删除与取消收藏传播（墓碑机制，30 天窗口）
- 最后写入者胜出 (LWW) 冲突解决
- 语义哈希比较，跳过无变更推送（避免同步循环）
- 冲突文件自动合并与清理
- 可配同步间隔（30秒 / 1分 / 10分 / 30分）+ 手动即时同步
- 仅收藏条目同步模式

### 设置
- 通用：开机自启、失焦隐藏、静默启动、主题模式、窗口位置、界面语言
- 剪贴板：排序、卡片高度、来源应用、自动滚顶、纯文本复制、悬停原文
- 快捷键：呼出快捷键录制、应用黑名单管理
- 数据：数据库路径自定义与迁移、最大保存条目数
- 同步：自动同步开关、间隔、仅收藏模式、多后端管理（增/删/改）

## 技术栈

| 组件 | 技术 |
|------|------|
| UI 框架 | [Slint](https://slint.dev/) 1.16 |
| 剪贴板 | [clipboard-rs](https://github.com/ChurchTao/clipboard-rs) |
| 数据存储 | [rusqlite](https://github.com/rusqlite/rusqlite) (bundled SQLite, WAL 模式) |
| 系统托盘 | [tray-icon](https://github.com/tauri-apps/tray-icon) |
| 全局快捷键 | [global-hotkey](https://github.com/tauri-apps/global-hotkey) |
| 图片处理 | [image](https://github.com/image-rs/image) |
| HTTP | [ureq](https://github.com/algesten/ureq) (favicon 获取) |
| 配置 | TOML ([serde](https://serde.rs/) + [toml](https://github.com/toml-rs/toml)) |
| 同步协议 | JSON (v1, [serde_json](https://github.com/serde-rs/json)) |
| Windows | [windows-sys](https://github.com/microsoft/windows-rs) |
| macOS | [objc2](https://github.com/madsmtm/objc2) + [core-graphics](https://github.com/servo/core-foundation-rs) |

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
| 界面语言 (中文/英文) | ✅ | ✅ |
